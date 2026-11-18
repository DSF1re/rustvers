//! Нативная отрисовка графа под Wayland — на частоте экрана.
//!
//! Зачем: WebKitGTK и GTK3 на части систем (например, экран на NVIDIA)
//! рисуют не чаще 60 раз в секунду. Граф выводится в собственную
//! `wl_subsurface` поверх области `#graph`, из отдельного потока (`surface.rs`),
//! а рисуется через femtovg (`view.rs`).
//!
//! Устройство:
//! * JS сообщает прямоугольник области графа и состояние (выбор, поиск…);
//! * GTK-события мыши над этой областью перехватываются до WebKit
//!   и уходят в поток отрисовки;
//! * поток отвечает событием `graph-open` и сменой курсора.
//!

mod surface;
mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use femtovg::{Canvas, Color, FontId, Renderer, renderer::OpenGl};
use gtk::gdk::EventMask;
use gtk::glib::Propagation;
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use tauri::{AppHandle, Emitter, Runtime, WebviewWindow};

use crate::core::graph::Graph;
use surface::{Parent, Surface};
use view::{Effect, GraphView};
pub use view::Options;

unsafe extern "C" {
    fn gdk_wayland_display_get_wl_display(display: *mut gtk::gdk::ffi::GdkDisplay) -> *mut c_void;
    fn gdk_wayland_window_get_wl_surface(window: *mut gtk::gdk::ffi::GdkWindow) -> *mut c_void;
}

// Виджеты GTK живут только в главном потоке.
thread_local! {
    static WEBVIEW: RefCell<Option<webkit2gtk::WebView>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<gtk::ApplicationWindow>> = const { RefCell::new(None) };
}

/// Прямоугольник области графа в CSS-пикселях окна WebView.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

enum Msg {
    Data(Graph, bool),
    /// Положение в координатах родительской поверхности и размер; `None` — скрыть.
    Layout(Option<(i32, i32, f32, f32)>, i32),
    Options(Options),
    Selected(Option<String>),
    Query(String),
    Theme(HashMap<String, String>),
    Fit,
    Fps(bool),
    Motion(f32, f32, u32),
    Press(f32, f32, u32),
    Release(u32),
    Leave,
    Scroll(f32, f32, f32),
}

#[derive(Default)]
struct Input {
    rect: Option<Rect>,
    dragging: bool,
    inside: bool,
}

impl Input {
    fn local(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        let r = self.rect?;
        let (x, y) = (x as f32 - r.x, y as f32 - r.y);
        (x >= 0.0 && y >= 0.0 && x < r.w && y < r.h).then_some((x, y))
    }
}

/// Ручка управления нативным графом (живёт в состоянии Tauri).
pub struct NativeGraph {
    tx: Sender<Msg>,
    input: Arc<Mutex<Input>>,
}

impl NativeGraph {
    /// Создаёт подповерхность и поток отрисовки. Вызывать из главного потока.
    pub fn start<R: Runtime>(app: &AppHandle<R>, window: &WebviewWindow<R>) -> Result<Self, String> {
        let win = window.gtk_window().map_err(|e| e.to_string())?;
        let parent = parent_of(&win).ok_or("окно не под Wayland")?;
        let scale = win.scale_factor();

        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let app = app.clone();
        std::thread::Builder::new()
            .name("graph-render".into())
            .spawn(move || render_thread(parent, scale, rx, ready_tx, app))
            .map_err(|e| e.to_string())?;
        ready_rx
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "поток отрисовки не ответил".to_string())??;

        let input = Arc::new(Mutex::new(Input::default()));
        WINDOW.with(|w| *w.borrow_mut() = Some(win));
        let (tx2, input2) = (tx.clone(), input.clone());
        window
            .with_webview(move |wv| {
                let view = wv.inner();
                hook_input(&view, tx2, input2);
                WEBVIEW.with(|w| *w.borrow_mut() = Some(view));
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, input })
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    /// `fresh` — новое хранилище (вписать в экран), иначе обновление
    /// после правки: раскладка продолжается с текущих позиций.
    pub fn set_data(&self, graph: Graph, fresh: bool) {
        self.send(Msg::Data(graph, fresh));
    }

    pub fn set_options(&self, o: Options) {
        self.send(Msg::Options(o));
    }

    pub fn set_selected(&self, id: Option<String>) {
        self.send(Msg::Selected(id));
    }

    pub fn set_query(&self, q: String) {
        self.send(Msg::Query(q));
    }

    pub fn set_theme(&self, colors: HashMap<String, String>) {
        self.send(Msg::Theme(colors));
    }

    pub fn fit(&self) {
        self.send(Msg::Fit);
    }

    pub fn show_fps(&self, on: bool) {
        self.send(Msg::Fps(on));
    }

    /// Новое положение области графа (`None` — граф скрыт). Главный поток.
    pub fn set_layout(&self, rect: Option<Rect>) {
        self.input.lock().unwrap().rect = rect;
        let geometry = WEBVIEW.with(|v| {
            WINDOW.with(|w| {
                let (v, w) = (v.borrow(), w.borrow());
                let (v, w) = (v.as_ref()?, w.as_ref()?);
                // Начало WebView внутри окна (заголовок, рамки и т.п.).
                let origin = v.translate_coordinates(w, 0, 0)?;
                Some((origin, w.scale_factor()))
            })
        });
        let Some(((ox, oy), scale)) = geometry else { return };
        let placed = rect
            .filter(|r| r.w >= 1.0 && r.h >= 1.0)
            .map(|r| (ox + r.x.round() as i32, oy + r.y.round() as i32, r.w.round(), r.h.round()));
        self.send(Msg::Layout(placed, scale));
    }
}

/// Указатели Wayland окна GTK; `None`, если окно не под Wayland.
fn parent_of(win: &gtk::ApplicationWindow) -> Option<Parent> {
    let gdk_window = win.window()?;
    let display = gdk_window.display();
    if display.type_().name() != "GdkWaylandDisplay" {
        return None;
    }
    // SAFETY: тип дисплея проверен выше; объекты живут, пока живо окно.
    unsafe {
        let d = gdk_wayland_display_get_wl_display(display.to_glib_none().0);
        let s = gdk_wayland_window_get_wl_surface(gdk_window.to_glib_none().0);
        (!d.is_null() && !s.is_null()).then_some(Parent { display: d as usize, surface: s as usize })
    }
}

// ---------------------------------------------------------------------------
// Ввод: события GTK над областью графа не доходят до WebKit
// ---------------------------------------------------------------------------

fn hook_input(view: &webkit2gtk::WebView, tx: Sender<Msg>, input: Arc<Mutex<Input>>) {
    view.add_events(
        EventMask::POINTER_MOTION_MASK
            | EventMask::BUTTON_PRESS_MASK
            | EventMask::BUTTON_RELEASE_MASK
            | EventMask::SCROLL_MASK
            | EventMask::SMOOTH_SCROLL_MASK
            | EventMask::LEAVE_NOTIFY_MASK,
    );

    let (t, i) = (tx.clone(), input.clone());
    view.connect_motion_notify_event(move |_, ev| {
        let (x, y) = ev.position();
        let mut s = i.lock().unwrap();
        if s.dragging {
            // Во время перетаскивания ведём указатель и за пределами области.
            if let Some(r) = s.rect {
                let _ = t.send(Msg::Motion(x as f32 - r.x, y as f32 - r.y, ev.time()));
            }
            return Propagation::Stop;
        }
        match s.local(x, y) {
            Some((lx, ly)) => {
                s.inside = true;
                let _ = t.send(Msg::Motion(lx, ly, ev.time()));
                Propagation::Stop
            }
            None => {
                if std::mem::take(&mut s.inside) {
                    let _ = t.send(Msg::Leave);
                }
                Propagation::Proceed
            }
        }
    });

    let (t, i) = (tx.clone(), input.clone());
    view.connect_button_press_event(move |_, ev| {
        if ev.button() != 1 {
            return Propagation::Proceed; // боковые кнопки — история в JS
        }
        let (x, y) = ev.position();
        let mut s = i.lock().unwrap();
        let Some((lx, ly)) = s.local(x, y) else { return Propagation::Proceed };
        if ev.event_type() == gtk::gdk::EventType::ButtonPress {
            s.dragging = true;
            let _ = t.send(Msg::Press(lx, ly, ev.time()));
        }
        Propagation::Stop
    });

    let (t, i) = (tx.clone(), input.clone());
    view.connect_button_release_event(move |_, ev| {
        let mut s = i.lock().unwrap();
        if ev.button() == 1 && std::mem::take(&mut s.dragging) {
            let _ = t.send(Msg::Release(ev.time()));
            return Propagation::Stop;
        }
        Propagation::Proceed
    });

    let (t, i) = (tx.clone(), input.clone());
    view.connect_scroll_event(move |_, ev| {
        let (x, y) = ev.position();
        let Some((lx, ly)) = i.lock().unwrap().local(x, y) else { return Propagation::Proceed };
        let dy = match ev.direction() {
            gtk::gdk::ScrollDirection::Up => -1.0,
            gtk::gdk::ScrollDirection::Down => 1.0,
            gtk::gdk::ScrollDirection::Smooth => ev.delta().1 as f32,
            _ => 0.0,
        };
        if dy != 0.0 {
            let _ = t.send(Msg::Scroll(lx, ly, dy));
        }
        Propagation::Stop
    });

    view.connect_leave_notify_event(move |_, _| {
        if std::mem::take(&mut input.lock().unwrap().inside) {
            let _ = tx.send(Msg::Leave);
        }
        Propagation::Proceed
    });
}

// ---------------------------------------------------------------------------
// Поток отрисовки
// ---------------------------------------------------------------------------

fn render_thread<R: Runtime>(
    parent: Parent,
    scale: i32,
    rx: Receiver<Msg>,
    ready: Sender<Result<(), String>>,
    app: AppHandle<R>,
) {
    // SAFETY: указатели GDK валидны всё время жизни окна.
    let mut surface = match unsafe { Surface::new(parent, 1, 1, scale) } {
        Ok(s) => s,
        Err(e) => return drop(ready.send(Err(e))),
    };
    let renderer = match unsafe { OpenGl::new_from_function(|name| surface.proc_address(name)) } {
        Ok(r) => r,
        Err(e) => return drop(ready.send(Err(format!("OpenGL: {e:?}")))),
    };
    let mut canvas = match Canvas::new(renderer) {
        Ok(c) => c,
        Err(e) => return drop(ready.send(Err(format!("canvas: {e:?}")))),
    };
    let font = load_font(&mut canvas);
    let _ = ready.send(Ok(()));

    let mut view = GraphView::new();
    let mut visible = false;
    let mut shown = false;
    let mut last = Instant::now();

    loop {
        let idle = !visible || (!view.busy() && !view.dirty);
        if idle {
            if shown && !visible {
                surface.hide();
                shown = false;
            }
            // Спим до следующей команды — ноль работы, пока ничего не меняется.
            let Ok(msg) = rx.recv() else { return };
            handle(msg, &mut view, &mut surface, &mut visible, &app);
            last = Instant::now();
        }
        while let Ok(msg) = rx.try_recv() {
            handle(msg, &mut view, &mut surface, &mut visible, &app);
        }
        dispatch_effects(&mut view, &app);
        if !visible {
            continue;
        }

        let now = Instant::now();
        view.step((now - last).as_secs_f32() * 1000.0);
        last = now;

        let (pw, ph) = surface.pixel_size();
        canvas.set_size(pw as u32, ph as u32, 1.0);
        view.draw(&mut canvas, font, surface.scale() as f32);
        canvas.flush();
        view.dirty = false;
        // Блокируется до сигнала композитора — темп = частота экрана.
        if let Err(e) = surface.present() {
            eprintln!("[graph] {e}");
            return;
        }
        shown = true;
        dispatch_effects(&mut view, &app);
    }
}

fn handle<R: Runtime>(msg: Msg, view: &mut GraphView, surface: &mut Surface, visible: &mut bool, app: &AppHandle<R>) {
    match msg {
        Msg::Data(g, fresh) => view.set_data(g, fresh),
        Msg::Layout(Some((x, y, w, h)), scale) => {
            surface.resize(w as i32, h as i32, scale);
            surface.set_position(x, y);
            view.set_size(w, h);
            *visible = true;
            view.dirty = true;
            // Положение подповерхности применяется с коммитом окна GTK.
            let _ = app.run_on_main_thread(|| {
                WINDOW.with(|w| {
                    if let Some(w) = w.borrow().as_ref() {
                        w.queue_draw();
                    }
                })
            });
        }
        Msg::Layout(None, _) => *visible = false,
        Msg::Options(o) => view.set_options(o),
        Msg::Selected(id) => view.set_selected(id),
        Msg::Query(q) => view.set_query(&q),
        Msg::Theme(colors) => {
            let t = &mut view.theme;
            for (key, slot) in [
                ("markdown", &mut t.markdown),
                ("html", &mut t.html),
                ("missing", &mut t.missing),
                ("edge", &mut t.edge),
                ("edgeHi", &mut t.edge_hi),
                ("label", &mut t.label),
                ("bg", &mut t.bg),
                ("accent", &mut t.accent),
            ] {
                if let Some(c) = colors.get(key).filter(|c| c.starts_with('#')) {
                    *slot = Color::hex(c);
                }
            }
            view.dirty = true;
        }
        Msg::Fit => view.fit(false),
        Msg::Fps(on) => {
            view.show_fps = on;
            view.dirty = true;
        }
        Msg::Motion(x, y, t) => view.pointer_move(x, y, t),
        Msg::Press(x, y, t) => view.pointer_down(x, y, t),
        Msg::Release(t) => view.pointer_up(t),
        Msg::Leave => view.pointer_leave(),
        Msg::Scroll(x, y, dy) => view.scroll(x, y, dy),
    }
}

fn dispatch_effects<R: Runtime>(view: &mut GraphView, app: &AppHandle<R>) {
    for effect in view.effects.drain(..) {
        match effect {
            Effect::Open(id) => {
                let _ = app.emit("graph-open", id);
            }
            Effect::Cursor(pointer) => {
                let _ = app.run_on_main_thread(move || {
                    WEBVIEW.with(|v| {
                        let v = v.borrow();
                        let Some(win) = v.as_ref().and_then(|v| v.window()) else { return };
                        let name = if pointer { "pointer" } else { "default" };
                        let cursor = gtk::gdk::Cursor::from_name(&win.display(), name);
                        win.set_cursor(cursor.as_ref());
                    })
                });
            }
        }
    }
}

/// Шрифт подписей: те же семейства, что в CSS интерфейса (`ui/style.css`),
/// первое реально установленное. `sans-serif` — только в крайнем случае:
/// у пользователя он может указывать на моноширинный шрифт.
fn load_font<T: Renderer>(canvas: &mut Canvas<T>) -> Option<FontId> {
    const FAMILIES: [&str; 5] = ["Inter", "Noto Sans", "DejaVu Sans", "Cantarell", "sans-serif"];
    let fc_match = |family: &str| -> Option<String> {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{family}\n%{file}", &format!("{family}:lang=ru")])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let text = String::from_utf8(out.stdout).ok()?;
        let (found, file) = text.split_once('\n')?;
        // fc-match всегда что-то возвращает — проверяем, что это то семейство.
        let exact = family == "sans-serif" || found.split(',').any(|f| f.eq_ignore_ascii_case(family));
        exact.then(|| file.to_string())
    };
    let candidates = FAMILIES.iter().filter_map(|f| fc_match(f)).chain(
        [
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
        .map(String::from),
    );
    for path in candidates {
        if let Ok(data) = std::fs::read(&path)
            && let Ok(id) = canvas.add_font_mem(&data) {
                return Some(id);
            }
    }
    eprintln!("[graph] не найден шрифт — подписи не будут показаны");
    None
}

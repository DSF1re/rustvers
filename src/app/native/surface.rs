//! Подповерхность Wayland с собственным контекстом OpenGL ES.
//!
//! GTK3 под Wayland (без сведений о частоте экрана) рисует не чаще 60 раз
//! в секунду, а WebKitGTK на некоторых драйверах — тоже. Поэтому граф
//! рисуется в отдельную `wl_subsurface` поверх окна, в своём потоке:
//! `eglSwapBuffers` с интервалом 1 ждёт сигнала композитора о следующем
//! кадре, так что темп задаёт сам композитор — на частоте экрана.

use std::ffi::c_void;

use khronos_egl as egl;
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry::WlRegistry,
    wl_subcompositor::WlSubcompositor, wl_subsurface::WlSubsurface, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_egl::WlEglSurface;

/// Ошибка с пояснением, на каком шаге она случилась.
fn fail<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{what}: {e}")
}

const EGL_PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

/// Указатели GDK, нужные для создания подповерхности.
#[derive(Clone, Copy)]
pub struct Parent {
    pub display: usize,
    pub surface: usize,
}

pub struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSubcompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlSubsurface);
delegate_noop!(State: ignore WlRegion);

/// Готовая к рисованию подповерхность.
pub struct Surface {
    queue: EventQueue<State>,
    egl: egl::DynamicInstance<egl::EGL1_5>,
    display: egl::Display,
    egl_surface: egl::Surface,
    _context: egl::Context,
    wl_egl: WlEglSurface,
    surface: WlSurface,
    subsurface: WlSubsurface,
    size: (i32, i32),
    scale: i32,
}

impl Surface {
    /// Создаёт подповерхность размера `w`×`h` (в логических пикселях).
    ///
    /// # Safety
    /// `parent` должен содержать живые `wl_display` и `wl_surface` GDK.
    pub unsafe fn new(parent: Parent, w: i32, h: i32, scale: i32) -> Result<Self, String> {
        // --- Wayland: своё соединение поверх дисплея GDK, своя очередь событий.
        let backend = unsafe { Backend::from_foreign_display(parent.display as *mut _) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(fail("реестр"))?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).map_err(fail("wl_compositor"))?;
        let subcompositor: WlSubcompositor = globals.bind(&qh, 1..=1, ()).map_err(fail("wl_subcompositor"))?;

        let parent_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), parent.surface as *mut _) }
            .map_err(fail("родительская поверхность"))?;
        let parent_surface = WlSurface::from_id(&conn, parent_id).map_err(fail("родительская поверхность"))?;

        let surface = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&surface, &parent_surface, &qh, ());
        // Свои кадры показываем сразу, не дожидаясь коммита GTK.
        subsurface.set_desync();
        // Ввод проходит насквозь — его обрабатывает GTK под нами.
        let region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        surface.set_buffer_scale(scale);

        let wl_egl = WlEglSurface::new(surface.id(), w * scale, h * scale).map_err(fail("wl_egl_window"))?;

        // --- EGL + OpenGL ES 2.
        let egl = unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required() }
            .map_err(fail("libEGL"))?;
        let display = unsafe {
            egl.get_platform_display(EGL_PLATFORM_WAYLAND_KHR, parent.display as *mut c_void, &[egl::ATTRIB_NONE])
        }
        .map_err(fail("EGL display"))?;
        egl.initialize(display).map_err(fail("eglInitialize"))?;
        let config = egl
            .choose_first_config(
                display,
                &[
                    egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8, egl::ALPHA_SIZE, 8,
                    egl::STENCIL_SIZE, 8,
                    egl::SURFACE_TYPE, egl::WINDOW_BIT,
                    egl::RENDERABLE_TYPE, egl::OPENGL_ES2_BIT,
                    egl::NONE,
                ],
            )
            .map_err(fail("eglChooseConfig"))?
            .ok_or("нет подходящей конфигурации EGL")?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(fail("eglBindAPI"))?;
        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])
            .map_err(fail("eglCreateContext"))?;
        let egl_surface = unsafe { egl.create_window_surface(display, config, wl_egl.ptr() as *mut c_void, None) }
            .map_err(fail("eglCreateWindowSurface"))?;
        egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(context))
            .map_err(fail("eglMakeCurrent"))?;
        // Интервал 1: swap ждёт композитора — это и есть vsync на частоте экрана.
        egl.swap_interval(display, 1).map_err(fail("eglSwapInterval"))?;

        Ok(Self {
            queue,
            egl,
            display,
            egl_surface,
            _context: context,
            wl_egl,
            surface,
            subsurface,
            size: (w, h),
            scale,
        })
    }

    /// Адрес функции OpenGL (для инициализации рендерера).
    pub fn proc_address(&self, name: &str) -> *const c_void {
        self.egl.get_proc_address(name).map_or(std::ptr::null(), |p| p as *const c_void)
    }

    /// Размер в физических пикселях.
    pub fn pixel_size(&self) -> (i32, i32) {
        (self.size.0 * self.scale, self.size.1 * self.scale)
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }

    /// Положение относительно родительской поверхности (логические пиксели).
    /// Применяется вместе со следующим коммитом родителя (GTK).
    pub fn set_position(&self, x: i32, y: i32) {
        self.subsurface.set_position(x, y);
    }

    pub fn resize(&mut self, w: i32, h: i32, scale: i32) {
        if (w, h, scale) == (self.size.0, self.size.1, self.scale) {
            return;
        }
        self.size = (w, h);
        if scale != self.scale {
            self.scale = scale;
            self.surface.set_buffer_scale(scale);
        }
        self.wl_egl.resize(w * scale, h * scale, 0, 0);
    }

    /// Показать готовый кадр. Блокируется до сигнала композитора.
    pub fn present(&mut self) -> Result<(), String> {
        let _ = self.queue.dispatch_pending(&mut State);
        self.egl
            .swap_buffers(self.display, self.egl_surface)
            .map_err(|e| format!("eglSwapBuffers: {e}"))
    }

    /// Убрать содержимое (подповерхность становится невидимой).
    pub fn hide(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        let _ = self.queue.flush();
    }
}

//! Снятие ограничения 60 fps в WebKitGTK.
//!
//! Начиная с 2.42, WebKitGTK по умолчанию держит `requestAnimationFrame`
//! около 60 Гц (флаг `PreferPageRenderingUpdatesNear60FPS`) даже на
//! мониторах 120/144/240 Гц. Отключаем этот флаг, чтобы граф рисовался на
//! частоте экрана, как в браузерах на Chromium.
//!
//! Этого недостаточно, если WebKit не может ждать vblank через DRM
//! (`drmWaitVBlank`): так бывает, например, когда экран подключён к NVIDIA
//! с проприетарным драйвером. Тогда WebKit переходит на таймер с жёсткими
//! 60 Гц, а GTK3 без сведений о частоте экрана тоже рисует раз в 16,7 мс.
//!
//! Rust-биндинги webkit2gtk 2.0 не покрывают API «features» (2.42+),
//! поэтому функции объявлены напрямую — библиотека уже слинкована Tauri.

use std::ffi::{CStr, c_char, c_int, c_void};

use webkit2gtk::WebViewExt;
use webkit2gtk::glib::translate::ToGlibPtr;

const FEATURE: &CStr = c"PreferPageRenderingUpdatesNear60FPS";

unsafe extern "C" {
    fn webkit_settings_get_all_features() -> *mut c_void;
    fn webkit_feature_list_get_length(list: *mut c_void) -> usize;
    fn webkit_feature_list_get(list: *mut c_void, index: usize) -> *mut c_void;
    fn webkit_feature_list_unref(list: *mut c_void);
    fn webkit_feature_get_identifier(feature: *mut c_void) -> *const c_char;
    fn webkit_settings_set_feature_enabled(settings: *mut c_void, feature: *mut c_void, enabled: c_int);
}

/// Возвращает `true`, если флаг найден и отключён.
pub fn unlock(webview: &webkit2gtk::WebView) -> bool {
    let Some(settings) = webview.settings() else { return false };
    let raw: *mut webkit2gtk::ffi::WebKitSettings = settings.to_glib_none().0;

    // SAFETY: список принадлежит нам до unref; элементы живут, пока жив список;
    // идентификатор — статическая C-строка WebKit; settings жив (держим `settings`).
    let found = unsafe {
        let list = webkit_settings_get_all_features();
        if list.is_null() {
            return false;
        }
        let mut found = false;
        for i in 0..webkit_feature_list_get_length(list) {
            let feature = webkit_feature_list_get(list, i);
            let id = webkit_feature_get_identifier(feature);
            if !id.is_null() && CStr::from_ptr(id) == FEATURE {
                webkit_settings_set_feature_enabled(raw.cast(), feature, 0);
                found = true;
                break;
            }
        }
        webkit_feature_list_unref(list);
        found
    };

    if found {
        // Переприменяем настройки, чтобы изменение дошло до веб-процесса.
        webview.set_settings(&settings);
    }
    found
}

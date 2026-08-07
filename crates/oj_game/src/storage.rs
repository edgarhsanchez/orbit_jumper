//! The persistence seam: RON strings keyed by name, stored wherever the
//! platform actually persists things — files beside the binary on
//! native, localStorage in the browser. Before this seam existed every
//! `std::fs` write silently no-opped on wasm, so "persistent" gear,
//! career and saves never survived a page reload on the web build.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    fn path(key: &str) -> String {
        format!("orbit_jumper_{key}.ron")
    }
    pub fn load(key: &str) -> Option<String> {
        std::fs::read_to_string(path(key)).ok()
    }
    pub fn save(key: &str, value: &str) {
        let _ = std::fs::write(path(key), value);
    }
    pub fn clear(key: &str) {
        let _ = std::fs::remove_file(path(key));
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    fn store() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }
    fn name(key: &str) -> String {
        format!("orbit_jumper_{key}")
    }
    pub fn load(key: &str) -> Option<String> {
        store()?.get_item(&name(key)).ok().flatten()
    }
    pub fn save(key: &str, value: &str) {
        if let Some(s) = store() {
            let _ = s.set_item(&name(key), value);
        }
    }
    pub fn clear(key: &str) {
        if let Some(s) = store() {
            let _ = s.remove_item(&name(key));
        }
    }
}

pub use imp::*;

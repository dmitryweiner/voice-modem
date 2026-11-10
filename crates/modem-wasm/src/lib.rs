//! Browser binding of the core. A thin adapter: logic and its tests live in
//! `modem-core`; this crate only converts types at the JS boundary.

use wasm_bindgen::prelude::*;

/// Version of the core.
#[wasm_bindgen]
pub fn version() -> String {
    modem_core::VERSION.to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_the_core_version() {
        assert_eq!(super::version(), modem_core::VERSION);
    }
}

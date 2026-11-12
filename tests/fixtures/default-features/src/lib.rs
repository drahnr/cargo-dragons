#[cfg(not(feature = "required"))]
compile_error!("the default feature set must remain enabled");

pub fn default_features_enabled() -> bool {
    cfg!(feature = "required")
}

#[cfg(not(any(feature = "backend-a", feature = "backend-b")))]
compile_error!("exactly one backend is required: enable backend-a or backend-b");

#[cfg(all(feature = "backend-a", feature = "backend-b"))]
compile_error!("backend-a and backend-b are mutually exclusive");

pub fn backend() -> &'static str {
    if cfg!(feature = "backend-a") {
        "backend-a"
    } else {
        "backend-b"
    }
}

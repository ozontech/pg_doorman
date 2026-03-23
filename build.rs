fn main() {
    // When PG_DOORMAN_STATIC is set, produce a fully static glibc binary.
    // The -static flag is passed to the linker only for the final binary crate,
    // not for proc-macros (which must remain dynamic cdylib).
    if std::env::var("PG_DOORMAN_STATIC").is_ok() {
        println!("cargo:rustc-link-arg=-static");
    }
    // Re-run if the env var changes.
    println!("cargo:rerun-if-env-changed=PG_DOORMAN_STATIC");
}

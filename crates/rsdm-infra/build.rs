fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    if pkg_config::probe_library("pam").is_err() {
        println!("cargo:rustc-link-lib=pam");
    }
    if std::env::var_os("CARGO_FEATURE_XSMP").is_some() {
        pkg_config::probe_library("sm").expect("the xsmp feature requires libSM development files");
        pkg_config::probe_library("ice").expect("the xsmp feature requires libICE development files");
    }
}

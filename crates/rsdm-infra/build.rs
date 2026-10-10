fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    if pkg_config::probe_library("pam").is_err() {
        println!("cargo:rustc-link-lib=pam");
    }
}

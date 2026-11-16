fn main() {
    glib_build_tools::compile_resources(&["data"], "data/gtube.gresource.xml", "gtube.gresource");
    // libmpv: pkg-config knows where it is (Homebrew, MSYS2, distro paths).
    if pkg_config::Config::new().atleast_version("2.0").probe("mpv").is_err() {
        println!("cargo:rustc-link-lib=mpv");
    }
}

fn main() {
    let library = pkg_config::Config::new()
        .probe("libwebp")
        .expect("libwebp development files are required");
    let mut build = cc::Build::new();
    build.file("src/webp.c");
    for path in library.include_paths {
        build.include(path);
    }
    build.compile("timeline_webp");
    println!("cargo:rerun-if-changed=src/webp.c");
}

fn main() {
    // Link against IOKit framework
    println!("cargo:rustc-link-lib=framework=IOKit");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    // CoreGraphics: cursor position + display bounds for `calibrate`
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
}

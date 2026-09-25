fn main() {
    // Rebuild native icon resources when the SVG-generated assets change.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}

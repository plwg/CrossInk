fn main() {
    let lib = "../../firmware/lib/Fsrs7";
    println!("cargo:rerun-if-changed={lib}/Fsrs7.cpp");
    println!("cargo:rerun-if-changed={lib}/Fsrs7.h");
    println!("cargo:rerun-if-changed=shim.cpp");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(lib)
        .file(format!("{lib}/Fsrs7.cpp"))
        .file("shim.cpp")
        .warnings(true)
        .extra_warnings(true)
        .flag_if_supported("-Werror")
        .compile("fsrs7_device");
}

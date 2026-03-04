use std::path::PathBuf;

fn main() {
    let c_src = PathBuf::from("c_src");

    // All doomgeneric C files we need (excluding platform backends we replaced)
    let sources: Vec<PathBuf> = std::fs::read_dir(&c_src)
        .expect("c_src directory must exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |e| e == "c"))
        // Exclude files we replace with our own shims
        .filter(|p| {
            let name = p.file_name().unwrap().to_str().unwrap();
            !matches!(
                name,
                "w_file_stdc.c"  // replaced by our fileio shim
                | "i_sound.c"    // replaced by our no-op sound
                | "i_cdmus.c"    // CD music — not needed
                | "libc_shim.c"  // compiled separately below
                | "doomgeneric_polkavm.c"
                | "fileio_shim.c"
                | "i_sound_stub.c"
            )
        })
        .collect();

    let mut build = cc::Build::new();

    // Override the target triple — clang doesn't understand "polkavm" env,
    // but riscv32-unknown-none-elf produces compatible object code.
    build
        .target("riscv32-unknown-none-elf")
        .files(&sources)
        // Our shim files
        .file("c_src/libc_shim.c")
        .file("c_src/doomgeneric_polkavm.c")
        .file("c_src/fileio_shim.c")
        .file("c_src/i_sound_stub.c")
        .include(&c_src)
        // Key defines
        .define("HAVE_DECL_STRCASECMP", "1")
        .define("HAVE_DECL_STRNCASECMP", "1")
        // Use 32-bit ARGB pixels (not 8-bit palette)
        .define("MODE_32BPP", None)
        // Disable features that need OS support
        .define("NO_SIGNAL_HANDLING", None)
        // Suppress warnings for old C code
        .warnings(false)
        .flag("-fno-builtin")
        .flag("-ffreestanding")
        // Ensure we generate rv32 code compatible with polkavm
        .flag("-march=rv32emc")
        .flag("-mabi=ilp32e")
        .flag("-nostdinc")
        // Prevent clang from emitting calls to compiler-rt builtins that don't exist
        .flag("-fno-stack-protector");

    build.compile("doom");

    println!("cargo:rerun-if-changed=c_src/");
}

"""
PlatformIO pre-build script: build inki-core (Rust) and link it into the
firmware.

The flashcard scheduler (FSRS-7), the .inki deck format and the parameter
file parser are shared with the inki PC tool as the Rust crate inki-core,
exposed to C/C++ by the inki-ffi crate (include/inki_core.h). This script
runs cargo for the board's CPU and adds the static library and header.

The inki repository is found at ../inki next to this one, or at $INKI_DIR.
Needs Rust (rustup) with the target for the board:
  ESP32-C3 (X3, X4):        rustup target add riscv32imc-unknown-none-elf
  ESP32-S3 (Sticky, X4 Pro): the Xtensa toolchain from espup (cargo +esp)
  simulator:                 the host toolchain
"""

import os
import shutil
import subprocess
import sys

Import("env")  # noqa: F821  (provided by PlatformIO)

TARGETS = {
    # mcu: (rust target, extra cargo arguments)
    "esp32c3": ("riscv32imc-unknown-none-elf", []),
    "esp32s3": ("xtensa-esp32s3-none-elf", ["-Zbuild-std=core"]),
}

# mcu: (PlatformIO toolchain package, binutils prefix)
BINUTILS = {
    "esp32c3": ("toolchain-riscv32-esp", "riscv32-esp-elf-"),
    "esp32s3": ("toolchain-xtensa-esp-elf", "xtensa-esp32s3-elf-"),
}


def fail(message):
    print(f"ERROR [build_inki_core.py]: {message}", file=sys.stderr)
    env.Exit(1)  # noqa: F821


def find_cargo():
    cargo = shutil.which("cargo")
    if cargo:
        return cargo
    fallback = os.path.expanduser("~/.cargo/bin/cargo")
    return fallback if os.path.isfile(fallback) else None


project_dir = env.subst("$PROJECT_DIR")  # noqa: F821
inki_dir = os.environ.get("INKI_DIR") or os.path.normpath(
    os.path.join(project_dir, "..", "inki")
)
crate_dir = os.path.join(inki_dir, "crates", "inki-ffi")
if not os.path.isfile(os.path.join(crate_dir, "Cargo.toml")):
    fail(
        f"inki-ffi not found in {inki_dir}. Clone the inki repository next to this one "
        "(../inki) or set INKI_DIR to its location."
    )

cargo = find_cargo()
if not cargo:
    fail("cargo not found. Install Rust from https://rustup.rs")

native = env.get("PIOPLATFORM") == "native"  # noqa: F821
if native:
    target, extra, toolchain = None, [], []
else:
    mcu = env.BoardConfig().get("build.mcu", "")  # noqa: F821
    if mcu not in TARGETS:
        fail(f"no Rust target known for MCU {mcu!r}")
    target, extra = TARGETS[mcu]
    toolchain = ["+esp"] if mcu == "esp32s3" else []

target_dir = os.path.join(env.subst("$BUILD_DIR"), "inki-ffi")  # noqa: F821
command = [
    cargo,
    *toolchain,
    "build",
    "--release",
    "--quiet",
    "--manifest-path",
    os.path.join(crate_dir, "Cargo.toml"),
    "--target-dir",
    target_dir,
    *(["--target", target] if target else []),
    *extra,
]
print(f"inki-core: building {target or 'host'} library from {crate_dir}")
if subprocess.run(command, check=False).returncode != 0:
    hint = f" (try: rustup target add {target})" if target and not toolchain else ""
    fail(f"cargo build of inki-ffi failed{hint}")

lib_dir = (
    os.path.join(target_dir, target, "release")
    if target
    else os.path.join(target_dir, "release")
)

if not native:
    # Rust's compiler_builtins carries fallback copies of C library functions
    # (pow, log, ...) for targets without a C library. Linked first, they
    # would replace the C library's versions for the whole firmware. Make
    # every plain C-named symbol except inki_* local to the Rust library.
    package, prefix = BINUTILS[mcu]
    tool_dir = os.path.join(env.PioPlatform().get_package_dir(package) or "", "bin")  # noqa: F821
    nm = os.path.join(tool_dir, prefix + "nm")
    objcopy = os.path.join(tool_dir, prefix + "objcopy")
    if not (os.path.isfile(nm) and os.path.isfile(objcopy)):
        fail(f"binutils not found in {tool_dir}")
    archive = os.path.join(lib_dir, "libinki_ffi.a")
    link_dir = os.path.join(target_dir, "link")
    os.makedirs(link_dir, exist_ok=True)
    linked = os.path.join(link_dir, "libinki_ffi.a")
    shutil.copyfile(archive, linked)
    names = subprocess.run(
        [nm, "-g", "--defined-only", archive],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    c_names = sorted(
        {
            n
            for n in names
            if n.replace("_", "").isalnum()
            and n[0].isalpha()
            and not n.startswith("inki_")
        }
    )
    if c_names:
        args = [objcopy]
        for name in c_names:
            args += ["--localize-symbol", name]
        if subprocess.run([*args, linked], check=False).returncode != 0:
            fail("objcopy could not localize C library symbols in libinki_ffi.a")
    lib_dir = link_dir

env.Append(  # noqa: F821
    CPPPATH=[os.path.join(crate_dir, "include")],
    LIBPATH=[lib_dir],
    LIBS=["inki_ffi"],
)

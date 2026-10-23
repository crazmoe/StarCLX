//! Baut libre und libbaresip (Git-Submodule unter `vendor/`, Version im
//! Submodul-Tag) statisch und die C-Schicht `src/shim.c` dazu.

use std::path::{Path, PathBuf};

/// In libbaresip einkompilierte Module. g722 braucht spandsp; aufile spielt
/// WAV-Dateien als Quelle ab (Ansagen von `sfctl say`).
const MODULES: &str =
    "g711;g722;srtp;auconv;auresamp;pipewire;pulse;alsa;stun;netroam;ausine;aubridge;aufile";

/// Systembibliotheken per pkg-config (Namen der .pc-Dateien).
const SYSTEM_LIBS: &[&str] = &["openssl", "zlib", "libpipewire-0.3", "libpulse", "alsa"];

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vendor = root.join("vendor");
    let re_src = vendor.join("re");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let baresip_vendor = vendor.join("baresip");

    if !re_src.join("CMakeLists.txt").exists() || !baresip_vendor.join("CMakeLists.txt").exists() {
        panic!("Submodule fehlen: `git submodule update --init --recursive` ausführen");
    }

    // baresip wird aus einer gepatchten Kopie gebaut, das Submodul bleibt
    // unverändert.
    let baresip_src = out.join("baresip-src");
    copy_tree(&baresip_vendor, &baresip_src);
    patch_baresip(&baresip_src);

    let re_out = cmake::Config::new(&re_src)
        .out_dir(out.join("re"))
        .profile("Release")
        .define("LIBRE_BUILD_SHARED", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .build_target("re")
        .build();
    let re_lib = find_lib(&re_out.join("build"), "libre.a");

    let baresip_out = cmake::Config::new(&baresip_src)
        .out_dir(out.join("baresip"))
        .profile("Release")
        .define("STATIC", "ON")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("MODULES", MODULES)
        .define("re_DIR", re_src.join("cmake"))
        .define("RE_LIBRARY", &re_lib)
        .define("RE_INCLUDE_DIR", re_src.join("include"))
        .build_target("baresip")
        .build();
    let baresip_lib = find_lib(&baresip_out.join("build"), "libbaresip.a");

    let shim_out = cmake::Config::new(root.join("cmake"))
        .out_dir(out.join("shim"))
        .profile("Release")
        .define("re_DIR", re_src.join("cmake"))
        .define("SFSIP_SHIM", root.join("src/shim.c"))
        .define("RE_INCLUDE", re_src.join("include"))
        .define("BARESIP_INCLUDE", baresip_vendor.join("include"))
        .build();

    println!(
        "cargo:rustc-link-search=native={}",
        shim_out.join("lib").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        baresip_lib.parent().unwrap().display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        re_lib.parent().unwrap().display()
    );
    // Reihenfolge: Nutzer vor Bibliothek.
    println!("cargo:rustc-link-lib=static=sfsip");
    println!("cargo:rustc-link-lib=static=baresip");
    println!("cargo:rustc-link-lib=static=re");

    // spandsp (nur für G.722) statisch, damit das Paket auf dem Zielsystem
    // nicht nachinstalliert werden muss. Die Fax-Teile, die libtiff
    // bräuchten, werden nicht mitgelinkt.
    let spandsp = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("spandsp")
        .unwrap_or_else(|e| panic!("spandsp nicht gefunden (libspandsp-dev installieren): {e}"));
    for dir in &spandsp.link_paths {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    println!("cargo:rustc-link-lib=static=spandsp");

    for lib in SYSTEM_LIBS {
        pkg_config::Config::new().probe(lib).unwrap_or_else(|e| {
            panic!("{lib} nicht gefunden (Entwicklungspaket installieren): {e}")
        });
    }
    for lib in ["resolv", "m", "dl", "pthread"] {
        println!("cargo:rustc-link-lib={lib}");
    }

    println!("cargo:rerun-if-changed=src/shim.c");
    println!("cargo:rerun-if-changed=cmake/CMakeLists.txt");
}

/// Ersetzungen im baresip-Quelltext: (Datei, alt, neu).
///
/// Das pipewire-Modul übergibt `pw_stream_connect` die Node-ID aus seinem
/// Register. Das Register vergisst entfernte Nodes nie und liefert für einen
/// Namen die älteste ID. Nach einem Bluetooth-Profilwechsel, dem Neustart
/// eines Loopbacks oder von PipeWire zeigt sie ins Leere, und WirePlumber
/// nimmt das Standardgerät. Mit `PW_ID_ANY` gilt allein `target.object`,
/// also der Node-Name, den baresip ohnehin setzt.
///
/// Ausserdem setzt das Modul `media.role = Communication`. WirePlumbers
/// `intended-roles.lua` legt solche Streams per `target.node` auf jedes
/// Gerät mit dieser Rolle, etwa die Freisprech-Nodes eines
/// Bluetooth-Headsets, und übergeht damit das gewählte Gerät. Die Rolle
/// gibt es deshalb nur noch ohne gewähltes Gerät.
const BARESIP_PATCHES: &[(&str, &str, &str)] = &[
    (
        "modules/pipewire/playback.c",
        "PW_DIRECTION_OUTPUT,\n\t\t\t\tpw_device_id(dev),",
        "PW_DIRECTION_OUTPUT,\n\t\t\t\tPW_ID_ANY,",
    ),
    (
        "modules/pipewire/capture.c",
        "PW_DIRECTION_INPUT,\n\t\t\t\tpw_device_id(dev),",
        "PW_DIRECTION_INPUT,\n\t\t\t\tPW_ID_ANY,",
    ),
    (
        "modules/pipewire/playback.c",
        "PW_KEY_MEDIA_ROLE, \"Communication\",",
        "PW_KEY_MEDIA_ROLE, str_isset(dev) ? NULL : \"Communication\",",
    ),
    (
        "modules/pipewire/capture.c",
        "PW_KEY_MEDIA_ROLE, \"Communication\",",
        "PW_KEY_MEDIA_ROLE, str_isset(dev) ? NULL : \"Communication\",",
    ),
];

fn patch_baresip(src: &Path) {
    for (file, old, new) in BARESIP_PATCHES {
        let path = src.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        if text.contains(new) {
            continue;
        }
        assert!(
            text.contains(old),
            "{file}: Patch passt nicht mehr zur baresip-Version"
        );
        std::fs::write(&path, text.replacen(old, new, 1)).unwrap();
    }
}

/// Kopiert `from` nach `to` ohne `.git`. Unveränderte Dateien bleiben
/// unberührt, damit CMake sie nicht neu übersetzt.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&src, &dst);
        } else if std::fs::read(&dst).ok() != Some(std::fs::read(&src).unwrap()) {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn find_lib(dir: &Path, name: &str) -> PathBuf {
    let candidates = [dir.join(name), dir.join("Release").join(name)];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("{name} nicht gebaut in {}", dir.display()))
}

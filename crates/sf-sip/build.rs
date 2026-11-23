//! Baut libre und libbaresip (Git-Submodule unter `vendor/`, Version im
//! Submodul-Tag) statisch und die C-Schicht `src/shim.c` dazu.
//!
//! Unter Windows (MSVC) kommt OpenSSL statisch aus vcpkg (`OPENSSL_DIR`,
//! z. B. `C:\vcpkg\installed\x64-windows-static-md`), G.722 aus libg722
//! (Submodul) statt spandsp, Audio über WASAPI.

use std::path::{Path, PathBuf};

/// In libbaresip einkompilierte Module. g722 braucht spandsp; aufile spielt
/// WAV-Dateien als Quelle ab (Ansagen von `sfctl say`).
const MODULES: &str =
    "g711;g722;srtp;auconv;auresamp;pipewire;pulse;alsa;stun;netroam;ausine;aubridge;aufile";

/// Unter Windows: G.722 über libg722, Audio über WASAPI
const MODULES_WINDOWS: &str =
    "g711;libg722;srtp;auconv;auresamp;wasapi;stun;netroam;ausine;aubridge;aufile";

/// Systembibliotheken per pkg-config (Namen der .pc-Dateien).
const SYSTEM_LIBS: &[&str] = &["openssl", "zlib", "libpipewire-0.3", "libpulse", "alsa"];

/// Windows-Bibliotheken, die re, baresip und OpenSSL brauchen
const WINDOWS_LIBS: &[&str] = &[
    "qwave", "iphlpapi", "wsock32", "ws2_32", "dbghelp", "winmm", "gdi32", "crypt32", "strmiids",
    "ole32", "oleaut32", "advapi32", "user32", "bcrypt", "shell32",
];

fn main() {
    // Nur Typprüfung (z. B. `cargo check` für Windows unter Linux): nichts
    // bauen, gelinkt wird dabei nicht.
    println!("cargo:rerun-if-env-changed=SFSIP_CHECK_ONLY");
    if std::env::var_os("SFSIP_CHECK_ONLY").is_some() {
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vendor = root.join("vendor");
    let re_src = vendor.join("re");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let baresip_vendor = vendor.join("baresip");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");

    if !re_src.join("CMakeLists.txt").exists() || !baresip_vendor.join("CMakeLists.txt").exists() {
        panic!("Submodule fehlen: `git submodule update --init --recursive` ausführen");
    }

    // baresip wird aus einer gepatchten Kopie gebaut, das Submodul bleibt
    // unverändert.
    let baresip_src = out.join("baresip-src");
    copy_tree(&baresip_vendor, &baresip_src);
    patch_baresip(&baresip_src);

    let openssl = windows.then(openssl_dir);

    let mut re_cfg = cmake::Config::new(&re_src);
    re_cfg
        .out_dir(out.join("re"))
        .profile("Release")
        .define("LIBRE_BUILD_SHARED", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .build_target("re");
    with_openssl(&mut re_cfg, openssl.as_deref());
    let re_out = re_cfg.build();
    let re_lib = find_lib(
        &re_out.join("build"),
        if windows { "re-static.lib" } else { "libre.a" },
    );

    let mut baresip_cfg = cmake::Config::new(&baresip_src);
    baresip_cfg
        .out_dir(out.join("baresip"))
        .profile("Release")
        .define("STATIC", "ON")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("MODULES", if windows { MODULES_WINDOWS } else { MODULES })
        .define("re_DIR", re_src.join("cmake"))
        .define("RE_LIBRARY", &re_lib)
        .define("RE_INCLUDE_DIR", re_src.join("include"))
        .build_target("baresip");
    with_openssl(&mut baresip_cfg, openssl.as_deref());
    if windows {
        let (include, lib) = build_libg722(&vendor.join("libg722"), &out);
        baresip_cfg
            .define("LIBG722_INCLUDE_DIR", include)
            .define("LIBG722_LIBRARY", lib);
    }
    let baresip_out = baresip_cfg.build();
    let baresip_lib = find_lib(
        &baresip_out.join("build"),
        if windows {
            "libbaresip.lib"
        } else {
            "libbaresip.a"
        },
    );

    let mut shim_cfg = cmake::Config::new(root.join("cmake"));
    shim_cfg
        .out_dir(out.join("shim"))
        .profile("Release")
        .define("re_DIR", re_src.join("cmake"))
        .define("SFSIP_SHIM", root.join("src/shim.c"))
        .define("RE_INCLUDE", re_src.join("include"))
        .define("BARESIP_INCLUDE", baresip_vendor.join("include"));
    with_openssl(&mut shim_cfg, openssl.as_deref());
    let shim_out = shim_cfg.build();

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
    if windows {
        println!("cargo:rustc-link-lib=static=libbaresip");
        println!("cargo:rustc-link-lib=static=re-static");
        println!("cargo:rustc-link-lib=static=g722");
        let lib = openssl.as_deref().unwrap().join("lib");
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=static=libssl");
        println!("cargo:rustc-link-lib=static=libcrypto");
        for lib in WINDOWS_LIBS {
            println!("cargo:rustc-link-lib={lib}");
        }
        println!("cargo:rerun-if-changed=src/shim.c");
        println!("cargo:rerun-if-changed=cmake/CMakeLists.txt");
        println!("cargo:rerun-if-env-changed=OPENSSL_DIR");
        return;
    }
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
    // Fedora und andere RPM-Distributionen liefern kein libspandsp.a; dort
    // für lokale Builds dynamisch linken (spandsp muss dann installiert sein).
    let default_dirs = [
        "/usr/lib64",
        "/usr/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ];
    let has_static = spandsp
        .link_paths
        .iter()
        .map(|p| p.as_path())
        .chain(default_dirs.iter().map(Path::new))
        .any(|d| d.join("libspandsp.a").exists());
    if has_static {
        println!("cargo:rustc-link-lib=static=spandsp");
    } else {
        println!("cargo:warning=libspandsp.a fehlt, spandsp wird dynamisch gelinkt");
        println!("cargo:rustc-link-lib=spandsp");
    }

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

/// OpenSSL für Windows: statische Bibliotheken samt Headern, z. B. aus
/// `vcpkg install openssl:x64-windows-static-md`.
fn openssl_dir() -> PathBuf {
    println!("cargo:rerun-if-env-changed=OPENSSL_DIR");
    let dir = std::env::var_os("OPENSSL_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("VCPKG_INSTALLATION_ROOT")
                .map(|v| PathBuf::from(v).join("installed/x64-windows-static-md"))
        })
        .unwrap_or_else(|| PathBuf::from(r"C:\vcpkg\installed\x64-windows-static-md"));
    assert!(
        dir.join("include/openssl/ssl.h").exists(),
        "OpenSSL nicht gefunden in {} (OPENSSL_DIR setzen, z. B. auf vcpkg \
         installed/x64-windows-static-md)",
        dir.display()
    );
    dir
}

/// Unter Windows OpenSSL statisch und ohne zlib (re nimmt es sonst, wenn
/// es zufällig eines findet).
fn with_openssl(cfg: &mut cmake::Config, openssl: Option<&Path>) {
    if let Some(dir) = openssl {
        cfg.define("OPENSSL_ROOT_DIR", dir)
            .define("OPENSSL_USE_STATIC_LIBS", "TRUE")
            .define("CMAKE_DISABLE_FIND_PACKAGE_ZLIB", "ON");
    }
}

/// libg722 als statische Bibliothek `g722`: (Header-Ordner, Bibliothek)
fn build_libg722(src: &Path, out: &Path) -> (PathBuf, PathBuf) {
    if !src.join("g722_codec.h").exists() {
        panic!("Submodul libg722 fehlt: `git submodule update --init --recursive` ausführen");
    }
    let dir = out.join("libg722");
    cc::Build::new()
        .files([src.join("g722_encode.c"), src.join("g722_decode.c")])
        .include(src)
        .warnings(false)
        .out_dir(&dir)
        .compile("g722");
    (src.to_path_buf(), dir.join("g722.lib"))
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
    // Bluetooth-Headsets: Das Mikrofon lässt sich erst öffnen, wenn Windows
    // auf das Freisprechprofil umgeschaltet hat; bis dahin (bis ~20 s) mit
    // neu geholtem Standardgerät nochmal versuchen
    (
        "modules/wasapi/src.c",
        r#"	hr = IMMDevice_Activate(capturer, &IID_IAudioClient, CLSCTX_ALL,
				NULL, (void **)&client);
	CHECK_HR(hr, "wasapi/src: IMMDevice_Activate failed");

	hr = IAudioClient_GetMixFormat(client, &format);
	CHECK_HR(hr, "wasapi/src: GetMixFormat failed");

	format->wFormatTag = WAVE_FORMAT_PCM;
	format->nChannels = st->prm.ch;
	format->nSamplesPerSec = st->prm.srate;
	format->wBitsPerSample = (WORD)aufmt_sample_size(st->prm.fmt) * 8;
	format->nBlockAlign = (format->wBitsPerSample / 8) * format->nChannels;
	format->nAvgBytesPerSec = format->nSamplesPerSec * format->nBlockAlign;
	format->cbSize = 0;

	hr = IAudioClient_Initialize(
		client, AUDCLNT_SHAREMODE_SHARED,
		AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
			AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
		(st->prm.ptime * REF_PER_MS * 2), 0, format, NULL);
	CHECK_HR(hr, "wasapi/src: IAudioClient_Initialize failed");
"#,
        r#"	int attempt;
	for (attempt = 1;; ++attempt) {
		hr = IMMDevice_Activate(capturer, &IID_IAudioClient, CLSCTX_ALL,
					NULL, (void **)&client);
		CHECK_HR(hr, "wasapi/src: IMMDevice_Activate failed");

		hr = IAudioClient_GetMixFormat(client, &format);
		CHECK_HR(hr, "wasapi/src: GetMixFormat failed");

		format->wFormatTag = WAVE_FORMAT_PCM;
		format->nChannels = st->prm.ch;
		format->nSamplesPerSec = st->prm.srate;
		format->wBitsPerSample = (WORD)aufmt_sample_size(st->prm.fmt) * 8;
		format->nBlockAlign = (format->wBitsPerSample / 8) * format->nChannels;
		format->nAvgBytesPerSec = format->nSamplesPerSec * format->nBlockAlign;
		format->cbSize = 0;

		hr = IAudioClient_Initialize(
			client, AUDCLNT_SHAREMODE_SHARED,
			AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
				AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
			(st->prm.ptime * REF_PER_MS * 2), 0, format, NULL);
		if (SUCCEEDED(hr) || attempt >= 40 ||
		    !re_atomic_rlx(&st->run))
			break;

		/* Bluetooth headsets fail while switching to hands-free */
		warning("wasapi/src: IAudioClient_Initialize failed: 0x%08x,"
			" retry %d\n", hr, attempt);
		CoTaskMemFree(format);
		format = NULL;
		IAudioClient_Release(client);
		client = NULL;
		sys_msleep(500);

		/* the endpoint may have been replaced meanwhile */
		if (pl_strcasecmp(st->device, "default") == 0) {
			IMMDevice_Release(capturer);
			capturer = NULL;
			hr = IMMDeviceEnumerator_GetDefaultAudioEndpoint(
				enumerator, eCapture, eCommunications,
				&capturer);
			CHECK_HR(hr, "wasapi/src: GetDefaultAudioEndpoint"
				 " failed");
		}
	}
	CHECK_HR(hr, "wasapi/src: IAudioClient_Initialize failed");
	if (attempt > 1)
		info("wasapi/src: started after %d attempts\n", attempt);
"#,
    ),
];

fn patch_baresip(src: &Path) {
    for (file, old, new) in BARESIP_PATCHES {
        let path = src.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        // Git unter Windows checkt je nach core.autocrlf mit CRLF aus
        let eol = |s: &str| {
            if text.contains("\r\n") {
                s.replace('\n', "\r\n")
            } else {
                s.to_owned()
            }
        };
        let (old, new) = (eol(old), eol(new));
        if text.contains(&new) {
            continue;
        }
        assert!(
            text.contains(&old),
            "{file}: Patch passt nicht mehr zur baresip-Version"
        );
        std::fs::write(&path, text.replacen(&old, &new, 1)).unwrap();
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

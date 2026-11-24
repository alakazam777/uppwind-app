// Uppwind Link — le programme qui charge les plugins VST 3 heberges, sur macOS (20 nov. 2026, relecture de securite).
//
// Ce n'est pas une autre application : il vit a cote du compagnon (dans le .dmg, puis dans le dossier ou --install le
// copie) et le compagnon le lance, une fois par plugin (--plug-host) et pour le scan (--plug-scan). Il fait
// exactement ce que fait `uppwind-companion --plug-host` (uwl_vst3host::child), rien d'autre : ni serveur, ni fichier
// d'etat, ni journal.
//
// Pourquoi a part : pour charger Diva, Serum… (signes par leur editeur), macOS exige du processus l'autorisation
// com.apple.security.cs.disable-library-validation. Elle est donnee a CE programme seulement (dev/host-entitlements.plist,
// .github/workflows/macos-build.yml) ; le compagnon, qui garde le code d'appairage, l'armement et les autorisations
// d'enregistrement de l'ecran et d'Accessibilite, reste signe sans aucune autorisation : il ne charge jamais le code d'un
// autre editeur. Ce n'est pas un bac a sable : un plugin tourne sous ton compte. Sous Windows rien ne change : le
// compagnon se relance lui-meme (host.rs).
//
// (24 nov., audit M5) Deux barrieres de plus, ici meme (pas seulement dans le compagnon) :
// - il ne charge un module que dans les deux dossiers VST 3 (/Library/Audio/Plug-Ins/VST3, ~/Library/Audio/Plug-Ins/VST3),
//   chemin resolu (liens suivis) : lance par un autre programme, il n'est pas un moyen de faire tourner n'importe quel
//   code sous la signature d'Uppwind ;
// - au demarrage, il se relance en place (meme processus, meme pid, memes tuyaux : posix_spawn POSIX_SPAWN_SETEXEC) en
//   renoncant a la « responsabilite » du compagnon (responsibility_spawnattrs_setdisclaim) : macOS ne lui prete plus les
//   autorisations du compagnon (Enregistrement de l'ecran, Accessibilite). Il n'en a pas besoin : le compagnon filme, et
//   les gestes passent par la file d'evenements de l'enfant. Mesure du 24 nov. sur macOS 26.6 : AXIsProcessTrusted
//   passe de 1 a 0 apres ce relancement, le pid reste le meme.

fn main() {
    #[cfg(target_os = "macos")]
    {
        let mut raw: Vec<String> = std::env::args().skip(1).collect();
        if raw.first().map(|s| s.as_str()) == Some("--version") {
            // (relecture 0.4.13) le compagnon verifie que c'est sa version (host.rs configure)
            println!("{}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        // (24 nov., relecture) la marque du relancement est un argument que seul le relancement ajoute (le compagnon lance
        // le helper avec des arguments fixes) ; une variable d'environnement pouvait arriver par le compagnon lui-meme
        if raw.first().map(|s| s.as_str()) == Some(mac::DISCLAIMED_ARG) {
            raw.remove(0);
        } else {
            mac::disclaim();
        }
        let code = match raw.first().map(|s| s.as_str()) {
            Some("--plug-scan") if raw.len() == 2 => {
                let p = std::path::Path::new(&raw[1]);
                if !mac::module_ok(p) {
                    println!("{}", serde_json::json!({ "ok": false, "error": mac::OUTSIDE }));
                    std::process::exit(2);
                }
                uwl_vst3host::child::scan_main(p)
            }
            Some("--plug-host") => match uwl_vst3host::child::HostArgs::parse(&raw[1..]) {
                Ok(a) if !mac::module_ok(&a.module) || (!a.effect && !mac::link_bundle_ok(&a.link_bundle)) => {
                    println!("{}", serde_json::json!({ "event": "error", "error": mac::OUTSIDE, "fatal": true }));
                    2
                }
                Ok(a) => uwl_vst3host::child::host_main(a),
                Err(e) => {
                    println!("{}", serde_json::json!({ "event": "error", "error": e, "fatal": true }));
                    2
                }
            },
            _ => {
                eprintln!("uppwind-link-host: Uppwind Link starts this program itself, to load VST 3 plugins; there is nothing to run here.");
                2
            }
        };
        std::process::exit(code);
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("uppwind-link-host: macOS only (on Windows, uppwind-companion hosts plugins itself).");
        std::process::exit(2);
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_void, CString};
    use std::path::{Path, PathBuf};

    pub const OUTSIDE: &str = "this plugin is not in the VST3 folders (/Library/Audio/Plug-Ins/VST3, ~/Library/Audio/Plug-Ins/VST3)";

    /// La marque d'un helper deja relance : le premier argument, ajoute seulement par le relancement. Un programme qui
    /// lancerait le helper avec elle n'y gagne rien : le helper aurait alors ses autorisations a lui (le responsable est
    /// celui qui le lance), jamais celles du compagnon, qui ne la passe jamais.
    pub const DISCLAIMED_ARG: &str = "--uwl-disclaimed";
    const POSIX_SPAWN_SETEXEC: i16 = 0x0040;

    extern "C" {
        fn posix_spawnattr_init(attr: *mut *mut c_void) -> i32;
        fn posix_spawnattr_setflags(attr: *mut *mut c_void, flags: i16) -> i32;
        fn posix_spawnattr_destroy(attr: *mut *mut c_void) -> i32;
        fn getuid() -> u32;
        fn getpwuid(uid: u32) -> *const Passwd;
        fn posix_spawn(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const *mut c_void, argv: *const *const c_char, envp: *const *const c_char) -> i32;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    /// Se relancer en place sans la responsabilite du compagnon (voir l'en-tete). Une seule fois ; un echec (une version de
    /// macOS sans ce symbole) : on continue tel quel, comme avant.
    pub fn disclaim() {
        if !try_disclaim() {
            // (24 nov., relecture) un echec se dit : le compagnon l'ecrit dans son journal (« notice »)
            println!("{}", serde_json::json!({ "event": "notice", "notice": "uppwind-link-host could not drop the companion's permissions (macOS refused the restart): plugins run with them" }));
        }
    }

    /// Le relancement : ne revient qu'en cas d'echec (false).
    fn try_disclaim() -> bool {
        let Ok(exe) = std::env::current_exe() else { return false };
        let Ok(path) = CString::new(exe.as_os_str().as_encoded_bytes()) else { return false };
        let mut os_args = std::env::args_os();
        let mut args: Vec<CString> = Vec::new();
        if let Some(a0) = os_args.next().and_then(|a| CString::new(a.as_encoded_bytes()).ok()) {
            args.push(a0);
        }
        args.push(CString::new(DISCLAIMED_ARG).expect("no NUL"));
        args.extend(os_args.filter_map(|a| CString::new(a.as_encoded_bytes()).ok()));
        let env: Vec<CString> = std::env::vars_os()
            .filter_map(|(k, v)| {
                let mut s = k.as_encoded_bytes().to_vec();
                s.push(b'=');
                s.extend_from_slice(v.as_encoded_bytes());
                CString::new(s).ok()
            })
            .collect();
        let argv: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
        let envp: Vec<*const c_char> = env.iter().map(|e| e.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
        // SAFETY : des chaines C vivantes jusqu'a l'appel, des tableaux termines par un pointeur nul ; le symbole est
        // cherche par dlsym (prive, mais present depuis macOS 10.14) et appele avec sa signature (attr*, int)
        unsafe {
            let sym = dlsym(-2isize as *mut c_void, c"responsibility_spawnattrs_setdisclaim".as_ptr());
            if sym.is_null() {
                return false;
            }
            let set_disclaim: extern "C" fn(*mut *mut c_void, i32) -> i32 = std::mem::transmute(sym);
            let mut attr: *mut c_void = std::ptr::null_mut();
            if posix_spawnattr_init(&mut attr) != 0 {
                return false;
            }
            if set_disclaim(&mut attr, 1) == 0 && posix_spawnattr_setflags(&mut attr, POSIX_SPAWN_SETEXEC) == 0 {
                let mut pid = 0i32;
                // avec SETEXEC, un succes ne revient pas : ce processus devient le nouveau ; un echec revient
                let _ = posix_spawn(&mut pid, path.as_ptr(), std::ptr::null(), &attr, argv.as_ptr(), envp.as_ptr());
            }
            posix_spawnattr_destroy(&mut attr);
            false
        }
    }

    /// `struct passwd` de macOS (<pwd.h>) : seul pw_dir sert.
    #[repr(C)]
    #[allow(dead_code)]
    struct Passwd {
        pw_name: *const c_char,
        pw_passwd: *const c_char,
        pw_uid: u32,
        pw_gid: u32,
        pw_change: i64,
        pw_class: *const c_char,
        pw_gecos: *const c_char,
        pw_dir: *const c_char,
        pw_shell: *const c_char,
        pw_expire: i64,
    }

    /// (24 nov., relecture) Le dossier personnel du compte, lu dans la base des comptes (getpwuid), pas dans HOME, qu'un
    /// programme qui lance le helper choisit.
    fn home_of_account() -> Option<PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY : getpwuid rend un pointeur vers une structure statique, ou nul ; pw_dir est une chaine C terminee
        unsafe {
            let p = getpwuid(getuid());
            if p.is_null() || (*p).pw_dir.is_null() {
                return None;
            }
            let dir = std::ffi::CStr::from_ptr((*p).pw_dir);
            Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
        }
    }

    fn roots() -> Vec<PathBuf> {
        let mut r = vec![PathBuf::from("/Library/Audio/Plug-Ins/VST3")];
        if let Some(h) = home_of_account() {
            r.push(h.join("Library/Audio/Plug-Ins/VST3"));
        }
        r.into_iter().filter_map(|p| p.canonicalize().ok()).collect()
    }

    /// Le module est-il dans un des deux dossiers VST 3 (chemin resolu, liens suivis) ? Une version de developpement
    /// (`uwl_vst3host::dev_build`) charge ce qu'on lui donne (les essais).
    pub fn module_ok(p: &Path) -> bool {
        if uwl_vst3host::dev_build() {
            return true;
        }
        inside(p, &roots())
    }

    /// Nos plugins (Notes, Return), dans le dossier VST 3 de l'utilisateur (plugin_install) : la meme regle.
    pub fn link_bundle_ok(p: &Path) -> bool {
        module_ok(p)
    }

    pub fn inside(p: &Path, roots: &[PathBuf]) -> bool {
        match p.canonicalize() {
            Ok(c) => roots.iter().any(|r| c.starts_with(r) && c != *r),
            Err(_) => false,
        }
    }
}

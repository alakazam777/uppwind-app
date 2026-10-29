// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les plugins dans un dossier temporaire (Target::in_dir) : jamais le dossier VST3 du systeme, jamais d'elevation.

use super::*;

fn temp_root(tag: &str) -> PathBuf {
    crate::setup::tests::temp_root(&format!("plugins-{tag}"))
}

/// Un faux bundle comme celui de macOS (plusieurs dossiers, un executable), v1 et v2.
static MAC_V1: &[(&str, &[u8], bool)] = &[
    ("Contents/Info.plist", b"<plist>v1</plist>", false),
    ("Contents/MacOS/Uppwind Link", b"\xcf\xfa\xed\xfe binary v1", true),
    ("Contents/PkgInfo", b"BNDL????", false),
    ("Contents/_CodeSignature/CodeResources", b"<seal>v1</seal>", false),
];
static MAC_V2: &[(&str, &[u8], bool)] = &[
    ("Contents/Info.plist", b"<plist>v2</plist>", false),
    ("Contents/MacOS/Uppwind Link", b"\xcf\xfa\xed\xfe binary v2, longer", true),
    ("Contents/PkgInfo", b"BNDL????", false),
    ("Contents/_CodeSignature/CodeResources", b"<seal>v2</seal>", false),
];
static WIN_V2: &[(&str, &[u8], bool)] = &[("Contents/x86_64-win/Uppwind Link.vst3", b"MZ the new plugin", false)];

fn set_marker_version(t: &Target, v: &str) {
    let mut m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&t.marker).unwrap()).unwrap();
    m["version"] = serde_json::json!(v);
    std::fs::write(&t.marker, m.to_string()).unwrap();
}

fn status(b: &Bundle, t: &Target) -> Status {
    inspect(b, t).status
}

#[test]
fn install_update_and_remove_in_a_test_folder() {
    let root = temp_root("cycle");
    let t = Target::in_dir(&root.join("VST3"));
    let (v1, v2) = (Bundle { files: MAC_V1 }, Bundle { files: MAC_V2 });
    // rien : a installer
    let mut r = inspect(&v1, &t);
    assert_eq!((r.status, r.installed_by.clone(), r.admin, r.simulated), (Status::Missing, None, false, false));
    plan(&mut r, false, false);
    assert_eq!(r.action, Action::Install);
    let a = apply(&mut r, &v1, &t, &|_| panic!("no administrator prompt for a test folder"));
    assert!(a.errors.is_empty() && !a.refused && !a.elevated && a.changed(), "{a:#?}");
    assert!(a.done[0].contains("installed in"), "{:?}", a.done);
    assert_eq!(r.after, Some(Status::UpToDate));
    for (rel, bytes, _) in MAC_V1 {
        assert_eq!(std::fs::read(path_in(&t.bundle(), rel)).unwrap(), *bytes, "{rel}");
    }
    // le releve : a cote, jamais dans le bundle
    assert!(t.marker.is_file() && !t.bundle().join(MARKER_NAME).exists());
    let r = inspect(&v1, &t);
    assert_eq!((r.status, r.installed_by.as_deref()), (Status::UpToDate, Some(env!("CARGO_PKG_VERSION"))));
    let mut again = r.clone();
    plan(&mut again, false, false);
    assert_eq!(again.action, Action::None, "up to date: nothing to do");
    // un compagnon plus recent porte v2 : vu d'ici, celui qu'on a mis est « plus ancien » si le releve le dit
    set_marker_version(&t, "0.0.1");
    assert_eq!(status(&v2, &t), Status::Outdated);
    set_marker_version(&t, "99.0.0");
    let mut r = inspect(&v2, &t);
    assert_eq!(r.status, Status::Newer);
    plan(&mut r, false, false);
    assert_eq!(r.action, Action::Skip, "never downgrade the plugins of a newer companion: {}", r.reason);
    plan(&mut r, false, true);
    assert_eq!(r.action, Action::Update, "--force");
    // modifie a la main : « different », remplace
    set_marker_version(&t, "0.0.1");
    std::fs::write(path_in(&t.bundle(), "Contents/PkgInfo"), b"edited").unwrap();
    assert_eq!(status(&v2, &t), Status::Different, "the files are no longer what the companion wrote");
    // un fichier en trop (une autre disposition d'avant) et le reste d'une installation : retires par la mise a jour
    let extra = path_in(&t.bundle(), "Contents/Resources/old.txt");
    std::fs::create_dir_all(extra.parent().unwrap()).unwrap();
    std::fs::write(&extra, "stale").unwrap();
    let left = path_in(&t.bundle(), "Contents/MacOS/Uppwind Link.old-1");
    std::fs::write(&left, "an old binary, free now").unwrap();
    let r = inspect(&v2, &t);
    assert!(r.files.iter().any(|f| f.name == "Contents/Resources/old.txt" && f.state == "extra"), "{:?}", r.files);
    assert_eq!(r.held, vec![left.clone()], "a leftover is listed apart, not as an extra file");
    let mut r = r;
    plan(&mut r, false, false);
    assert_eq!(r.action, Action::Update);
    let a = apply(&mut r, &v2, &t, &|_| {});
    assert!(a.errors.is_empty() && a.done[0].contains("updated in"), "{a:#?}");
    assert!(!extra.exists() && !extra.parent().unwrap().exists() && !left.exists(), "extra files, empty folders and free leftovers go");
    assert_eq!(inspect(&v2, &t).status, Status::UpToDate);
    // retirer : seulement ce que le compagnon a mis
    let mut r = inspect(&v2, &t);
    plan(&mut r, true, false);
    assert_eq!(r.action, Action::Remove);
    let a = apply(&mut r, &v2, &t, &|_| {});
    assert!(a.errors.is_empty() && a.done[0].starts_with("Removed the Uppwind Link plugins"), "{a:#?}");
    assert!(!t.bundle().exists() && !t.marker.exists(), "the bundle and the record are gone");
    assert!(root.join("VST3").is_dir(), "the VST3 folder itself stays (other plugins live there)");
    let mut r = inspect(&v2, &t);
    plan(&mut r, true, false);
    assert_eq!(r.action, Action::None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn plugins_the_companion_did_not_install_are_left_alone() {
    let root = temp_root("foreign");
    let t = Target::in_dir(&root.join("VST3"));
    let b = Bundle { files: MAC_V1 };
    // un bundle « a la main » (uwl-bundle --install, une autre version), sans releve
    let mine = path_in(&t.bundle(), "Contents/MacOS/Uppwind Link");
    std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
    std::fs::write(&mine, "a build of my own").unwrap();
    let mut r = inspect(&b, &t);
    assert_eq!((r.status, r.installed_by.clone()), (Status::Different, None));
    plan(&mut r, true, false);
    assert_eq!(r.action, Action::Skip, "{}", r.reason);
    assert!(apply(&mut r, &b, &t, &|_| {}).done.is_empty());
    assert!(mine.is_file(), "not removed");
    // installer par-dessus : remplace (c'est ce qu'on demande), puis le retrait le prend
    let mut r = inspect(&b, &t);
    plan(&mut r, false, false);
    assert_eq!(r.action, Action::Update);
    apply(&mut r, &b, &t, &|_| {});
    let mut r = inspect(&b, &t);
    plan(&mut r, true, false);
    assert_eq!(r.action, Action::Remove);
    // un releve qui nomme un autre dossier ne compte pas
    let other = Target::in_dir(&root.join("Other"));
    std::fs::create_dir_all(&other.vst3).unwrap();
    std::fs::copy(&t.marker, &other.marker).unwrap();
    assert_eq!(inspect(&b, &other).installed_by, None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_build_without_plugins_installs_nothing_but_can_remove_them() {
    let root = temp_root("empty");
    let t = Target::in_dir(&root.join("VST3"));
    let none = Bundle { files: &[] };
    let mut r = inspect(&none, &t);
    assert!(!r.embedded && r.status == Status::Missing);
    plan(&mut r, false, false);
    assert_eq!(r.action, Action::Skip);
    assert!(r.reason.contains("not in this build"), "{}", r.reason);
    assert!(write_bundle(&none, &t.bundle()).is_err());
    // une release les a mis ; une construction de developpement les retire quand meme (le releve)
    let b = Bundle { files: MAC_V1 };
    let mut r = inspect(&b, &t);
    plan(&mut r, false, false);
    apply(&mut r, &b, &t, &|_| {});
    let mut r = inspect(&none, &t);
    plan(&mut r, true, false);
    assert_eq!(r.action, Action::Remove, "{}", r.reason);
    apply(&mut r, &none, &t, &|_| {});
    assert!(!t.bundle().exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_trial_writes_nothing() {
    let root = temp_root("simulate");
    let mut t = Target::in_dir(&root.join("VST3"));
    t.simulate = true;
    let b = Bundle { files: MAC_V1 };
    let mut r = inspect(&b, &t);
    assert!(r.simulated);
    plan(&mut r, false, false);
    let a = apply(&mut r, &b, &t, &|_| panic!("no prompt"));
    assert!(a.done[0].contains("simulated"), "{:?}", a.done);
    assert!(!root.join("VST3").exists() && !t.marker.exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn leftover_names() {
    for n in ["Contents/x86_64-win/Uppwind Link.vst3.old-1", "Contents/MacOS/Uppwind Link.old-12", "Contents/PkgInfo.uwl-new-4242"] {
        assert!(is_leftover(n), "{n}");
    }
    for n in ["Contents/x86_64-win/Uppwind Link.vst3", "Contents/a.old-", "Contents/a.old-x", ".old-1", "Contents/notes.old-1.txt"] {
        assert!(!is_leftover(n), "{n}");
    }
}

#[test]
fn targets_never_point_at_the_system_folder_in_a_test() {
    let root = temp_root("targets");
    let t = Target::in_dir(&root.join("VST3"));
    assert_eq!(t.bundle(), root.join("VST3").join("Uppwind Link.vst3"));
    assert_eq!(t.marker, root.join("VST3").join(MARKER_NAME));
    assert!(!t.system && !t.simulate);
    // la fausse machine (UWC_SETUP_ROOT) : sous son dossier
    let paths = crate::setup::env::Paths::in_root(&root);
    if dir_override().is_none() {
        let f = target_for(&paths, true, false).unwrap();
        assert!(f.vst3.starts_with(&root) && !f.system, "{f:?}");
        assert_eq!(f.marker, paths.local.join("Uppwind Link").join(MARKER_NAME));
    }
    // le vrai dossier (lu, jamais ecrit ici) : Windows, le dossier connu Common Files ; macOS, ~/Library
    let real = system_vst3(Path::new("/Users/x")).unwrap();
    if cfg!(windows) {
        assert!(real.ends_with(r"Common Files\VST3") && real.is_absolute(), "{}", real.display());
    } else if cfg!(target_os = "macos") {
        assert_eq!(real, PathBuf::from("/Users/x/Library/Audio/Plug-Ins/VST3"));
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_elevated_copy_takes_exactly_its_flag_and_never_runs_in_a_test() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(elevate::main(&s(&["--plugins-elevated"])), elevate::USAGE);
    assert_eq!(elevate::main(&s(&["--plugins-elevated", "format"])), elevate::USAGE);
    assert_eq!(elevate::main(&s(&["--plugins-elevated", "install", "--plugins-dir", "C:\\x"])), elevate::USAGE, "no folder from the command line");
    assert_eq!(elevate::main(&s(&["--setup", "install"])), elevate::USAGE);
    // le bon drapeau : sous cfg(test), la copie elevee refuse d'ecrire (le dossier du systeme)
    let code = elevate::main(&s(&["--plugins-elevated", "install"]));
    assert!(code == elevate::FAILED || code == elevate::USAGE, "{code}");
    assert!(matches!(elevate::run("install"), elevate::Outcome::Failed(_)), "the tests never ask for administrator rights");
}

#[test]
fn the_embedded_bundle_has_the_layout_of_this_system() {
    let b = Bundle::embedded();
    if b.is_empty() {
        assert!(notices().is_empty(), "notices only travel with the plugins");
        return;
    }
    let names: Vec<&str> = b.files.iter().map(|(n, _, _)| *n).collect();
    if cfg!(windows) {
        assert_eq!(names, vec!["Contents/x86_64-win/Uppwind Link.vst3"]);
        assert_eq!(&b.files[0].1[..2], b"MZ");
    } else if cfg!(target_os = "macos") {
        assert_eq!(names, vec!["Contents/Info.plist", "Contents/MacOS/Uppwind Link", "Contents/PkgInfo", "Contents/_CodeSignature/CodeResources"]);
        assert!(b.files.iter().all(|(n, _, x)| *x == n.starts_with("Contents/MacOS/")));
    }
    assert!(notices().contains("Uppwind Link plugins"), "the plugins' notices are embedded with them");
}

/// Un dossier (ou une jonction) qui designe un autre endroit : jamais suivi, ni pour ecrire, ni pour effacer.
#[test]
fn links_inside_the_bundle_are_never_followed() {
    let root = temp_root("links");
    let t = Target::in_dir(&root.join("VST3"));
    let b = Bundle { files: MAC_V1 };
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("precious.txt"), "keep me").unwrap();
    let link = |from: &Path, to: &Path| -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(to, from).is_ok()
        }
        #[cfg(windows)]
        {
            // une jonction : pas besoin de droits particuliers (un lien symbolique, si)
            std::process::Command::new(crate::wizard::system_cmd())
                .args(["/d", "/c", "mklink", "/J"])
                .arg(from)
                .arg(to)
                .stdout(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        }
    };
    // 1. le bundle lui-meme est un lien : refuse
    std::fs::create_dir_all(&t.vst3).unwrap();
    assert!(link(&t.bundle(), &outside), "cannot make a link here");
    let e = write_bundle(&b, &t.bundle()).unwrap_err();
    assert!(e.text().contains("not a plain folder"), "{e:?}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1, "nothing written through the link");
    // le retirer ne retire que le lien
    let rm = remove_bundle(&t.bundle()).unwrap();
    assert!(rm.gone && outside.join("precious.txt").is_file());
    // 2. un lien a l'interieur, a la place d'un dossier du bundle : refuse
    std::fs::create_dir_all(t.bundle().join("Contents")).unwrap();
    assert!(link(&t.bundle().join("Contents").join("MacOS"), &outside));
    assert!(write_bundle(&b, &t.bundle()).unwrap_err().text().contains("not a plain folder"));
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    // 3. un lien en trop : retire lui-meme, jamais ce qu'il designe
    remove_entry(&t.bundle().join("Contents").join("MacOS")).unwrap();
    assert!(link(&t.bundle().join("Contents").join("Elsewhere"), &outside));
    let w = write_bundle(&b, &t.bundle()).unwrap();
    assert_eq!(w.removed, 1, "{w:?}");
    assert!(!t.bundle().join("Contents").join("Elsewhere").exists());
    assert_eq!(std::fs::read_to_string(outside.join("precious.txt")).unwrap(), "keep me");
    // 4. retirer le bundle n'entre pas dans un lien
    assert!(link(&t.bundle().join("Contents").join("Elsewhere"), &outside));
    assert!(remove_bundle(&t.bundle()).unwrap().gone);
    assert_eq!(std::fs::read_to_string(outside.join("precious.txt")).unwrap(), "keep me");
    let _ = std::fs::remove_dir_all(&root);
}

/// Windows : une DLL chargee (ici mappee comme image, comme le fait un DAW) ne s'efface ni ne s'ecrase, mais se renomme.
#[cfg(windows)]
#[test]
fn a_dll_a_daw_holds_is_set_aside_then_swept() {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_AS_IMAGE_RESOURCE};
    let root = temp_root("held");
    let t = Target::in_dir(&root.join("VST3"));
    // une vraie DLL comme « ancienne version » (celle de Windows : elle se mappe comme une image)
    let sys = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("version.dll");
    let old: &'static [u8] = Box::leak(std::fs::read(&sys).unwrap().into_boxed_slice());
    let old_files: &'static [(&'static str, &'static [u8], bool)] = Box::leak(vec![("Contents/x86_64-win/Uppwind Link.vst3", old, false)].into_boxed_slice());
    let (v1, v2) = (Bundle { files: old_files }, Bundle { files: WIN_V2 });
    write_bundle(&v1, &t.bundle()).unwrap();
    let dll = path_in(&t.bundle(), "Contents/x86_64-win/Uppwind Link.vst3");
    let hold = || unsafe { LoadLibraryExW(&HSTRING::from(dll.as_os_str()), None, LOAD_LIBRARY_AS_IMAGE_RESOURCE).unwrap() };
    let h = hold();
    assert!(std::fs::write(&dll, b"x").is_err() && std::fs::remove_file(&dll).is_err(), "Windows keeps a loaded DLL");
    // la mise a jour : l'ancienne de cote, la neuve a sa place
    let w = write_bundle(&v2, &t.bundle()).unwrap();
    assert_eq!(w.held.len(), 1, "{w:?}");
    assert!(w.held[0].to_string_lossy().ends_with("Uppwind Link.vst3.old-1"), "{:?}", w.held);
    assert_eq!(std::fs::read(&dll).unwrap(), WIN_V2[0].1);
    let r = inspect(&v2, &t);
    assert_eq!((r.status, r.held.len()), (Status::UpToDate, 1), "up to date, an old copy still held");
    // encore tenue : la reinstallation la laisse ; liberee : elle part
    assert_eq!(write_bundle(&v2, &t.bundle()).unwrap().held.len(), 1);
    unsafe { FreeLibrary(h).unwrap() };
    let w = write_bundle(&v2, &t.bundle()).unwrap();
    assert!(w.held.is_empty() && w.written == 0 && w.same == 1, "{w:?}");
    assert_eq!(std::fs::read_dir(dll.parent().unwrap()).unwrap().count(), 1, "only the plugin is left");
    // le retrait pendant qu'un DAW la tient : de cote, le reste parti ; liberee : tout part
    write_bundle(&v1, &t.bundle()).unwrap();
    let h = hold();
    let rm = remove_bundle(&t.bundle()).unwrap();
    assert!(!rm.gone && rm.held.len() == 1, "{rm:?}");
    assert!(!dll.exists(), "the DAW no longer finds a plugin binary there");
    unsafe { FreeLibrary(h).unwrap() };
    assert!(remove_bundle(&t.bundle()).unwrap().gone);
    assert!(!t.bundle().exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// Windows : un dossier ou l'on ne peut pas ecrire (ici, un refus pose sur un dossier d'essai) ; le dossier « du
/// systeme » passe alors par la copie elevee, ici une FAUSSE (jamais d'invite) : invite refusee, echec, succes annonce
/// mais rien d'ecrit. Hors du dossier du systeme : une erreur, jamais d'elevation.
#[cfg(windows)]
#[test]
fn a_folder_one_cannot_write_goes_through_the_elevated_copy_only_for_the_system_folder() {
    use std::cell::RefCell;
    let root = temp_root("denied");
    let vst3 = root.join("VST3");
    std::fs::create_dir_all(&vst3).unwrap();
    let user = std::env::var("USERNAME").unwrap();
    let icacls = |args: &[&str]| {
        let sys = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("icacls.exe");
        std::process::Command::new(sys).arg(&vst3).args(args).stdout(std::process::Stdio::null()).status().unwrap().success()
    };
    // (W) : ni fichier ni dossier neuf dans ce dossier, pour ce compte ; retire a la fin
    assert!(icacls(&["/deny", &format!("{user}:(W)")]));
    let b = Bundle { files: WIN_V2 };
    let calls = RefCell::new(Vec::new());
    let mut t = Target::in_dir(&vst3);
    let run = |t: &Target, want: elevate::Outcome| {
        let mut r = inspect(&b, t);
        plan(&mut r, false, false);
        let lines = RefCell::new(Vec::new());
        let a = apply_with(&mut r, &b, t, &|l| lines.borrow_mut().push(l.to_string()), &|op| {
            calls.borrow_mut().push(op.to_string());
            want.clone()
        });
        (a, lines.into_inner())
    };
    let (a, _) = run(&t, elevate::Outcome::Exit(elevate::OK));
    assert!(matches!(write_bundle(&b, &t.bundle()), Err(WriteError::Denied(_))), "the deny works");
    assert!(!a.errors.is_empty() && !a.elevated && calls.borrow().is_empty(), "a test folder never asks: {a:#?}");
    t.system = true;
    let (a, lines) = run(&t, elevate::Outcome::Refused);
    assert!(a.refused && a.errors.is_empty() && a.elevated, "{a:#?}");
    assert!(a.done[0].contains("refused") && lines[0].contains("administrator rights"), "{a:#?} {lines:?}");
    assert_eq!(calls.borrow().as_slice(), ["install"]);
    let (a, _) = run(&t, elevate::Outcome::Exit(elevate::FAILED));
    assert!(a.errors[0].contains("exit code 1"), "{a:#?}");
    let (a, _) = run(&t, elevate::Outcome::Exit(elevate::OK));
    assert!(a.errors[0].contains("does not hold this companion's plugins"), "a success that wrote nothing is caught: {a:#?}");
    assert!(icacls(&["/remove:d", &user]));
    let _ = std::fs::remove_dir_all(&root);
}

/// macOS (et Unix) : le binaire en 0755, le reste en 0644 ; un binaire qui a perdu son bit d'execution est reecrit ;
/// un dossier ou l'on ne peut pas ecrire donne « Denied » (sans dossier du systeme : pas d'elevation).
#[cfg(unix)]
#[test]
fn modes_and_a_folder_one_cannot_write() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_root("modes");
    let t = Target::in_dir(&root.join("VST3"));
    let b = Bundle { files: MAC_V1 };
    write_bundle(&b, &t.bundle()).unwrap();
    let mode = |rel: &str| std::fs::metadata(path_in(&t.bundle(), rel)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode("Contents/MacOS/Uppwind Link"), 0o755);
    assert_eq!(mode("Contents/Info.plist"), 0o644);
    std::fs::set_permissions(path_in(&t.bundle(), "Contents/MacOS/Uppwind Link"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(inspect(&b, &t).status, Status::Different);
    let w = write_bundle(&b, &t.bundle()).unwrap();
    assert_eq!((w.written, mode("Contents/MacOS/Uppwind Link")), (1, 0o755));
    let ro = Target::in_dir(&root.join("ReadOnly"));
    std::fs::create_dir_all(&ro.vst3).unwrap();
    std::fs::set_permissions(&ro.vst3, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert!(matches!(write_bundle(&b, &ro.bundle()), Err(WriteError::Denied(_))));
    let mut r = inspect(&b, &ro);
    plan(&mut r, false, false);
    let a = apply(&mut r, &b, &ro, &|_| panic!("no prompt outside the system folder"));
    assert!(!a.errors.is_empty() && !a.elevated, "{a:#?}");
    std::fs::set_permissions(&ro.vst3, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

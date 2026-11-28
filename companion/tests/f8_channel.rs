// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (audit F8, deuxieme relecture) Le canal du protocole de l'enfant hote, sous Windows, sur l'exe que cargo compile
//! pour ces essais (CARGO_BIN_EXE_uppwind-companion : toujours a jour, aussi sur la CI ou `cargo test` passe avant
//! `cargo build`). Le cote compagnon (host.rs, proto_of) est refait ici : un tuyau non heritable, son bout d'ecriture
//! copie dans l'enfant (DuplicateHandle), sa valeur en premiere ligne de l'entree standard.

#![cfg(windows)]

use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command, Stdio};

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_uppwind-companion")
}

fn give_channel(child: &mut Child) -> std::io::PipeReader {
    use windows::Win32::Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let (r, w) = std::io::pipe().unwrap();
    let mut theirs = HANDLE::default();
    unsafe { DuplicateHandle(GetCurrentProcess(), HANDLE(w.as_raw_handle()), HANDLE(child.as_raw_handle()), &mut theirs, 0, false, DUPLICATE_SAME_ACCESS) }.unwrap();
    drop(w);
    let stdin = child.stdin.as_mut().unwrap();
    writeln!(stdin, "{{\"proto\":{}}}", theirs.0 as usize).unwrap();
    stdin.flush().unwrap();
    r
}

/// Un scan : la reponse et ce que l'enfant a ecrit sur sa sortie standard (le compagnon la met sur NUL ; ici en
/// tuyau, pour voir que rien du protocole n'y passe).
fn scan(module: &str) -> (String, Vec<u8>) {
    let mut child = Command::new(exe()).arg("--plug-scan").arg(module).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut proto = give_channel(&mut child);
    drop(child.stdin.take());
    let mut got = String::new();
    proto.read_to_string(&mut got).unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();
    (got, out)
}

#[test]
fn the_protocol_comes_through_its_own_pipe_and_stdout_is_elsewhere() {
    let (got, out) = scan(r"C:\nowhere\none.vst3");
    assert!(got.contains("\"ok\":false"), "the scan's answer on the protocol pipe: {got:?}");
    assert!(out.is_empty(), "nothing of the protocol on the child's stdout: {:?}", String::from_utf8_lossy(&out));
}

/// Sans la premiere ligne du compagnon, l'enfant ne charge rien et sort.
#[test]
fn a_child_without_its_channel_loads_nothing() {
    let st = Command::new(exe()).arg("--plug-scan").arg(r"C:\nowhere\none.vst3").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output().unwrap();
    assert_eq!(st.status.code(), Some(2));
    assert!(st.stdout.is_empty(), "{:?}", String::from_utf8_lossy(&st.stdout));
}

/// A la main : UWL_F8_MODULE=<un .vst3> cargo test --release --test f8_channel -- --ignored : un vrai scan.
#[test]
#[ignore]
fn real_module_scan_keeps_stdout_apart() {
    let m = std::env::var("UWL_F8_MODULE").expect("UWL_F8_MODULE");
    let (got, out) = scan(&m);
    eprintln!("protocol: {} bytes; stdout: {} bytes: {:?}", got.len(), out.len(), String::from_utf8_lossy(&out[..out.len().min(200)]));
    assert!(got.contains("\"ok\""), "an answer on the protocol pipe: {got:?}");
    assert!(!String::from_utf8_lossy(&out).contains("\"ok\""), "the protocol leaked to stdout");
}

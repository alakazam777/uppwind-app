// Uppwind Link — le programme qui charge les plugins VST 3 heberges, sur macOS (20 nov. 2026, relecture de securite).
//
// Ce n'est pas une autre application : il vit a cote du compagnon (dans le .dmg, puis dans le dossier ou --install le
// copie) et seul le compagnon le lance, une fois par plugin (--plug-host) et pour le scan (--plug-scan). Il fait
// exactement ce que fait `uppwind-companion --plug-host` (uwl_vst3host::child), rien d'autre : ni serveur, ni fichier
// d'etat, ni journal.
//
// Pourquoi a part : pour charger Diva, Serum… (signes par leur editeur), macOS exige du processus l'autorisation
// com.apple.security.cs.disable-library-validation. Elle est donnee a CE programme seulement (dev/host-entitlements.plist,
// .github/workflows/macos-build.yml) ; le compagnon, qui garde le code d'appairage, l'armement et les autorisations
// d'enregistrement de l'ecran et d'Accessibilite, reste signe sans aucune autorisation : il ne charge jamais le code d'un
// autre editeur. Ce n'est pas un bac a sable : un plugin tourne sous ton compte, et macOS laisse sans doute ce programme,
// enfant du compagnon, se servir de ses autorisations (SECURITY.md). Sous Windows rien ne change : le compagnon se relance lui-meme (host.rs).

fn main() {
    #[cfg(target_os = "macos")]
    {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        let code = match raw.first().map(|s| s.as_str()) {
            // (relecture 0.4.13) le compagnon verifie que c'est sa version (host.rs configure)
            Some("--version") => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                0
            }
            Some("--plug-scan") if raw.len() == 2 => uwl_vst3host::child::scan_main(std::path::Path::new(&raw[1])),
            Some("--plug-host") => match uwl_vst3host::child::HostArgs::parse(&raw[1..]) {
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

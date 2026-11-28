// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (11 nov.) La mise a jour du compagnon, depuis sa fenetre d'etat : la seule connexion du compagnon a internet.
//!
//! - Une seule destination : les releases du depot public GitHub, demandees par son numero et pas par son nom
//!   (`api.github.com/repositories/<REPO_ID>/releases/latest`). (28 nov.) Le nom d'un compte GitHub renomme redevient
//!   libre : une adresse faite avec l'ancien nom suit la redirection de GitHub tant que personne ne reprend ce nom,
//!   puis mene chez celui qui l'a pris. Le numero du depot ne change ni au renommage du compte ni a celui du depot.
//!   Les fichiers sont telecharges a l'adresse que cette reponse donne pour chacun (`browser_download_url`), seulement
//!   de la forme `https://github.com/<compte>/<depot>/releases/download/<tag>/<nom>` (`asset_url`), que GitHub redirige
//!   vers ses serveurs de fichiers. L'adresse finale de chaque reponse (apres les redirections) doit etre sur un hote
//!   GitHub de la liste `HOSTS`, en https (WinHTTP refuse https → http).
//! - Rien n'est envoye : des GET sans corps, sans cookie, sans jeton ; les en-tetes poses ici : User-Agent
//!   « Uppwind-Link-Companion/<version> » (GitHub l'exige), Accept, et pour l'API X-GitHub-Api-Version (WinHTTP ajoute
//!   les siens, Host...). GitHub voit l'adresse IP, comme pour tout telechargement.
//! - Jamais sans un clic : « Check for updates », ou la case « Update automatically » (off par defaut, gardee dans le
//!   fichier d'etat, « autoUpdate »), cochee dans la fenetre d'etat seulement (jamais par le studio : ipc.rs).
//! - (24 nov., audit E2) Qui l'a publie : `SHA256SUMS` doit porter une signature, `SHA256SUMS.minisig` (format
//!   minisign : Ed25519 sur le BLAKE2b-512 du fichier, plus un « commentaire de confiance » signe lui aussi), faite par
//!   une des cles de `UPDATE_KEYS`, gardees hors ligne par Lucas, jamais sur GitHub. Le commentaire signe doit etre
//!   exactement « uppwind-companion <tag> » : une signature d'une autre release ne sert pas pour celle-ci. La signature
//!   est verifiee sur les octets telecharges de SHA256SUMS, AVANT de telecharger l'exe ; sans elle, rien n'est
//!   telecharge. Quelqu'un qui tiendrait le compte GitHub sans tenir la cle ne peut plus rien installer.
//! - Puis l'exe (non signe pour Windows, RELEASING.md etape 6) est verifie par son SHA-256, celui du fichier
//!   `SHA256SUMS` signe, ET celui que l'API de GitHub donne pour le fichier (`digest`) quand elle le donne,
//!   qui doivent etre egaux ; sa taille, celle de l'API ; ses deux premiers octets « MZ » ; (11 nov.) la version de son
//!   bloc de version, celle du tag. Le SHA-256 etant signe, ce numero de version l'est aussi : une ancienne release
//!   signee ne peut pas se faire passer pour une plus recente.
//! - Seulement une version plus recente (pas de retour en arriere), d'un tag `v<x>.<y>.<z>` ; (11 nov.) `--self-update`
//!   refuse aussi de remplacer une copie installee dont le fichier dit une version egale ou plus recente.
//! - L'exe verifie est ecrit dans `<dossier de l'application>\updates\`, puis lance avec `--self-update` : il s'installe
//!   comme le ferait sa fenetre d'installation (wizard::self_update : la copie, les scripts deja poses, l'entree
//!   « Applications installees »), arrete celui-ci et se lance a sa place (`--background --replace`).

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// (28 nov.) Le numero du depot public (`gh api repos/kaonaya-studio/uppwind-app --jq .id`) : il ne change pas quand
/// le compte ou le depot change de nom.
pub const REPO_ID: &str = "1434113356";
/// (28 nov.) La derniere release, demandee par le numero du depot (jamais par un nom de compte).
pub const API_LATEST: &str = "https://api.github.com/repositories/1434113356/releases/latest";
/// La page des versions, ouverte dans le navigateur (bouton de la fenetre d'etat, Mac) : un lien montre a la personne,
/// rien n'y est telecharge par le compagnon. GitHub n'a pas de page web par numero de depot (404) : le nom actuel.
pub const RELEASES_PAGE: &str = "https://github.com/kaonaya-studio/uppwind-app/releases/latest";
/// Le nom de l'exe Windows dans la release.
pub const EXE_NAME: &str = "uppwind-companion.exe";
pub const SUMS_NAME: &str = "SHA256SUMS";
/// (24 nov., audit E2) La signature de SHA256SUMS (minisign), dans la meme release.
pub const SIG_NAME: &str = "SHA256SUMS.minisig";
/// (24 nov., audit E2) Les cles publiques qui signent les mises a jour (minisign, « RW… », la 2e ligne d'un fichier
/// .pub de `minisign -G`) : la principale et celle de secours, deux au plus. Lucas garde les cles privees hors ligne.
/// Changer de cle : une release signee par l'ancienne apporte la nouvelle liste (RELEASING.md, « The update keys »).
/// Vide : cette version ne peut verifier aucune mise a jour (elle les refuse toutes), et release.yml refuse de la
/// publier (`--update-keys`). La CI ne controle que la forme des cles, pas a qui elles sont : avant de signer, Lucas
/// compare la sortie de `--update-keys` a ses deux fichiers .pub (RELEASING.md, etape 8). Une cle ajoutee ici par
/// quelqu'un d'autre, meme en 2e position, lui suffirait pour signer seul les mises a jour suivantes.
pub const UPDATE_KEYS: &[&str] = &[
    // (25 nov. 2026) la principale, puis celle de secours
    "RWRiZOJqi15ZkH0OAGn16SThSNwUDqa1LB6T41qaTwkg7opKbDeuelfC",
    "RWSCGE/9/XbxDIjmtwEAITlk/r4cnxTaktLS6ogpqT0KIJ4kk8jhYc+y",
];
/// Les hotes ou une reponse peut finir (apres les redirections de GitHub vers ses serveurs de fichiers).
pub const HOSTS: &[&str] = &["api.github.com", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
/// L'option de lancement du nouvel exe : un contrat entre les versions (toute version publiee doit la comprendre).
pub const SELF_UPDATE_FLAG: &str = "--self-update";
/// La case automatique : une verification au plus a ce rythme (et la premiere un peu apres le demarrage).
pub const AUTO_EVERY: Duration = Duration::from_secs(6 * 3600);
pub const AUTO_FIRST: Duration = Duration::from_secs(90);
/// Un studio connecte : la mise a jour automatique attend (elle couperait la session).
pub const AUTO_RETRY: Duration = Duration::from_secs(30 * 60);
/// Le cle du fichier d'etat.
pub const STATE_KEY: &str = "autoUpdate";
/// (24 nov., audit E2) La case « Update automatically » n'est proposee que si cette version connait une cle de
/// signature : sans elle, l'empreinte viendrait de la meme release que l'exe, et quelqu'un qui tiendrait le compte
/// GitHub installerait sans un clic son propre programme sur chaque ordinateur. Une case cochee par une version d'avant
/// ne fait rien tant qu'elle n'est pas proposee.
pub const AUTO_AVAILABLE: bool = !UPDATE_KEYS.is_empty();

const MAX_API: usize = 1024 * 1024;
const MAX_SUMS: usize = 16 * 1024;
const MAX_SIG: usize = 4 * 1024;
const MAX_EXE: usize = 96 * 1024 * 1024;

/// La derniere release, telle que l'API la decrit (seulement ce qui sert).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Release {
    pub tag: String,
    /// x.y.z (le tag sans « v »)
    pub version: String,
    /// la taille de l'exe selon l'API
    pub exe_size: u64,
    /// son SHA-256 selon l'API (« digest »: « sha256:… »), en hexadecimal minuscule ; None : l'API ne le donne pas
    pub exe_sha256: Option<String>,
    /// (28 nov.) les adresses de ses fichiers, prises dans la reponse de l'API (`asset_url`)
    #[serde(skip)]
    pub urls: Urls,
}

/// (28 nov.) Les adresses de telechargement d'une release : celles que l'API donne (`browser_download_url`), verifiees
/// par `asset_url`. Vides : une release qui ne vient pas de l'API (`--verify-release`), qui ne telecharge rien.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Urls {
    pub exe: String,
    pub sums: String,
    pub sig: String,
}

impl Release {
    pub fn exe_url(&self) -> &str {
        &self.urls.exe
    }
    pub fn sums_url(&self) -> &str {
        &self.urls.sums
    }
    pub fn sig_url(&self) -> &str {
        &self.urls.sig
    }
}

/// « 0.3.0 », « v0.3.0 » → (0, 3, 0) ; rien d'autre (ni suffixe, ni espace).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut it = s.split('.');
    let mut n = || -> Option<u32> {
        let p = it.next()?;
        if p.is_empty() || p.len() > 6 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let v = (n()?, n()?, n()?);
    if it.next().is_some() {
        return None;
    }
    Some(v)
}

/// `latest` est-elle plus recente que `current` ?
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// La reponse de `releases/latest` → la release, si elle porte l'exe Windows et SHA256SUMS.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "GitHub's answer is not JSON".to_string())?;
    let tag = v["tag_name"].as_str().ok_or("GitHub's answer has no tag_name")?;
    if parse_version(tag).is_none() || !tag.starts_with('v') {
        return Err(format!("the latest release has an unexpected tag ({})", tag.chars().take(40).collect::<String>()));
    }
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Err("the latest release is a draft or a pre-release".into());
    }
    let assets = v["assets"].as_array().ok_or("the latest release lists no files")?;
    let find = |name: &str| assets.iter().find(|a| a["name"].as_str() == Some(name));
    let exe = find(EXE_NAME).ok_or_else(|| format!("the latest release ({tag}) has no {EXE_NAME}"))?;
    let Some(sums) = find(SUMS_NAME) else {
        return Err(format!("the latest release ({tag}) has no {SUMS_NAME}: it cannot be checked, so it is not installed"));
    };
    let Some(sig) = find(SIG_NAME) else {
        return Err(format!("the latest release ({tag}) is not signed (no {SIG_NAME}): it is not installed"));
    };
    let exe_size = exe["size"].as_u64().filter(|s| *s > 0 && *s as usize <= MAX_EXE).ok_or("the release's exe has no plausible size")?;
    let exe_sha256 = match exe["digest"].as_str() {
        None => None,
        Some(d) => Some(d.strip_prefix("sha256:").filter(|h| is_hex64(h)).map(|h| h.to_ascii_lowercase()).ok_or("the release's exe has an unexpected digest")?),
    };
    let urls = Urls { exe: asset_url(exe, tag, EXE_NAME)?, sums: asset_url(sums, tag, SUMS_NAME)?, sig: asset_url(sig, tag, SIG_NAME)? };
    Ok(Release { tag: tag.to_string(), version: tag[1..].to_string(), exe_size, exe_sha256, urls })
}

/// (28 nov.) L'adresse de telechargement d'un fichier de la release, celle que l'API donne (`browser_download_url`),
/// seulement de la forme `https://github.com/<compte>/<depot>/releases/download/<tag>/<nom>` : ce tag, ce nom, rien
/// apres (ni `?`, ni `#`, ni `..`). Le compte et le depot sont ceux que GitHub dit pour le numero `REPO_ID`, quels
/// qu'ils soient : ce qui garantit l'auteur, c'est la signature, pas l'adresse.
fn asset_url(asset: &Value, tag: &str, name: &str) -> Result<String, String> {
    let bad = || format!("the release's {name} has an unexpected download address: nothing is installed");
    let url = asset["browser_download_url"].as_str().ok_or_else(|| format!("the release's {name} has no download address: nothing is installed"))?;
    let path = url.strip_prefix("https://github.com/").filter(|_| url.len() <= 300).ok_or_else(bad)?;
    let parts: Vec<&str> = path.split('/').collect();
    let segment = |s: &str| !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    match parts.as_slice() {
        [owner, repo, "releases", "download", t, n] if segment(owner) && segment(repo) && *t == tag && *n == name && allowed_url(url) => Ok(url.to_string()),
        _ => Err(bad()),
    }
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Le SHA-256 de `name` dans un fichier SHA256SUMS (« <64 hex>  <nom> » ou « <64 hex> *<nom> », une ligne par fichier).
/// Deux lignes differentes pour le meme nom : aucune (ambigu).
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in sums.lines() {
        let line = line.trim_end_matches('\r');
        let Some((h, rest)) = line.split_once(' ') else { continue };
        let n = rest.trim_start_matches(' ').trim_start_matches('*');
        if n == name && is_hex64(h) {
            let h = h.to_ascii_lowercase();
            if found.as_ref().is_some_and(|f| *f != h) {
                return None;
            }
            found = Some(h);
        }
    }
    found
}

/// L'exe telecharge est-il celui de la release ? → son SHA-256.
pub fn verify(bytes: &[u8], r: &Release, sums: &str) -> Result<String, String> {
    let want = sum_for(sums, EXE_NAME).ok_or_else(|| format!("{SUMS_NAME} of {} has no line for {EXE_NAME}", r.tag))?;
    if let Some(api) = &r.exe_sha256 {
        if *api != want {
            return Err(format!("GitHub and {SUMS_NAME} disagree on the SHA-256 of {EXE_NAME}: nothing is installed"));
        }
    }
    if bytes.len() as u64 != r.exe_size {
        return Err(format!("the download is {} bytes, the release says {}: nothing is installed", bytes.len(), r.exe_size));
    }
    if !bytes.starts_with(b"MZ") {
        return Err("the download is not a Windows program: nothing is installed".into());
    }
    let got = crate::plugin_install::sha256::hex(bytes);
    if got != want {
        return Err(format!("the SHA-256 of the download ({}…) is not the release's ({}…): nothing is installed", &got[..12], &want[..12]));
    }
    Ok(got)
}

/// Le nom du .dmg de macOS dans la release (dans SHA256SUMS quand il y est ; `--verify-release` le verifie aussi).
pub const DMG_NAME: &str = "uppwind-companion-macos.dmg";

/// (24 nov., audit E2) Le commentaire signe d'une release : « uppwind-companion v0.4.16 » (`minisign -t`).
pub fn signed_comment(tag: &str) -> String {
    format!("uppwind-companion {tag}")
}

/// Un texte venu de la release, pour un message : des caracteres visibles seulement, 60 au plus.
fn clip(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(60).collect()
}

/// (24 nov., audit E2) `sig` (le fichier SHA256SUMS.minisig) est-il une signature des octets `sums` (SHA256SUMS tel
/// que telecharge, jamais relu ou reconverti avant) par une des cles `keys`, faite pour la release `tag` ? → le rang
/// de la cle (0 : la premiere). Refuse : aucune cle connue, une signature mal formee, de l'ancien mode minisign (non
/// pre-hache), d'une cle inconnue, fausse (le fichier ou le commentaire change), ou faite pour une autre release.
pub fn verify_signature(sums: &[u8], sig: &[u8], tag: &str, keys: &[&str]) -> Result<usize, String> {
    use minisign_verify::{Error as E, PublicKey, Signature};
    if keys.is_empty() {
        return Err(format!("this version of Uppwind Link knows no signing key, so it cannot check {tag}: nothing is installed. Get the new version from its release page, {RELEASES_PAGE}"));
    }
    let bad = || format!("the signature of {tag} ({SIG_NAME}) is malformed: nothing is installed");
    let text = std::str::from_utf8(sig).map_err(|_| bad())?;
    let signature = Signature::decode(text).map_err(|_| bad())?;
    for (i, k) in keys.iter().enumerate() {
        let pk = PublicKey::from_base64(k).map_err(|_| format!("signing key {} of this version is malformed: nothing is installed", i + 1))?;
        match pk.verify(sums, &signature, false) {
            Ok(()) => {
                // (le commentaire est couvert par la signature : il n'est lu qu'une fois celle-ci verifiee)
                let got = signature.trusted_comment();
                if got != signed_comment(tag) {
                    return Err(format!("the signature was made for another release (\"{}\"), not {tag}: nothing is installed", clip(got)));
                }
                return Ok(i);
            }
            Err(E::UnexpectedKeyId) => continue,
            Err(E::UnexpectedAlgorithm) => return Err(format!("the signature of {tag} uses minisign's old mode, which is refused: nothing is installed")),
            Err(_) => return Err(format!("the signature of {tag} is false ({SUMS_NAME} or the signature was changed, or not signed by Uppwind): nothing is installed")),
        }
    }
    Err(format!("{tag} is signed with a key this version does not know: nothing is installed"))
}

/// Une adresse finale est-elle permise ? (https, un hote de HOSTS, rien avant l'hote)
pub fn allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.contains(['@', ':']) && HOSTS.iter().any(|h| host.eq_ignore_ascii_case(h))
}

// ── l'etat de la mise a jour (pour la fenetre d'etat et l'icone) ──────────────────────────────────────────

#[derive(Default)]
struct Inner {
    checking: bool,
    /// depuis quand une mise a jour est lancee (elle remplace ce processus ; si rien ne s'est passe en 3 min, on peut
    /// reessayer)
    installing: Option<Instant>,
    found: Option<(Release, Instant)>,
    checked: Option<Instant>,
    error: Option<String>,
}

static STATE: Mutex<Inner> = Mutex::new(Inner { checking: false, installing: None, found: None, checked: None, error: None });

const INSTALL_WAIT: Duration = Duration::from_secs(180);

impl Inner {
    fn installing(&self) -> bool {
        self.installing.is_some_and(|t| t.elapsed() < INSTALL_WAIT)
    }
}

fn state() -> std::sync::MutexGuard<'static, Inner> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ce que la fenetre d'etat montre de la mise a jour.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct View {
    /// cette machine sait s'installer la nouvelle version toute seule (Windows) ; sinon la page de la release
    pub supported: bool,
    /// la case « Update automatically »
    pub auto: bool,
    /// (24 nov., audit E2) la case est-elle proposee (AUTO_AVAILABLE)
    pub auto_available: bool,
    pub checking: bool,
    pub installing: bool,
    /// la derniere version publiee, si on l'a lue
    pub latest: Option<String>,
    /// elle est plus recente que celle-ci
    pub newer: bool,
    pub checked_ago_s: Option<u64>,
    pub error: Option<String>,
    /// la derniere mise a jour faite par `--self-update` (son fichier de compte rendu)
    pub last: Option<Value>,
}

pub fn view(state_file: Option<&Path>, updates_dir: Option<&Path>) -> View {
    let s = state();
    View {
        supported: cfg!(windows),
        auto: state_file.is_some_and(auto_on),
        auto_available: AUTO_AVAILABLE,
        checking: s.checking,
        installing: s.installing(),
        latest: s.found.as_ref().map(|(r, _)| r.version.clone()),
        newer: s.found.as_ref().is_some_and(|(r, _)| is_newer(&r.version, env!("CARGO_PKG_VERSION"))),
        checked_ago_s: s.checked.map(|t| t.elapsed().as_secs()),
        error: s.error.clone(),
        last: updates_dir.and_then(|d| crate::state_file::read(&d.join(LAST_NAME))).filter(|v| v["version"].is_string()),
    }
}

/// « autoUpdate » du fichier d'etat.
pub fn auto_on(state_file: &Path) -> bool {
    AUTO_AVAILABLE && auto_ticked(state_file)
}

/// La case telle que le fichier d'etat la garde (sans tenir compte de AUTO_AVAILABLE).
pub fn auto_ticked(state_file: &Path) -> bool {
    crate::state_file::read(state_file).is_some_and(|v| v[STATE_KEY] == Value::Bool(true))
}

pub fn set_auto(state_file: &Path, on: bool) -> Result<bool, String> {
    if on && !AUTO_AVAILABLE {
        return Err("Automatic updates are off until updates are signed: use Check for updates, then Update.".into());
    }
    crate::state_file::update(state_file, |v| {
        v[STATE_KEY] = Value::Bool(on);
        true
    })
    .map(|_| on)
    .map_err(|e| format!("could not keep the choice in {} ({e})", state_file.display()))
}

/// Ce qu'une verification a trouve.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub release: Release,
    pub newer: bool,
}

/// Lit la derniere release (une requete a l'API de GitHub). Une seule verification a la fois.
pub fn check_now() -> Result<Check, String> {
    {
        let mut s = state();
        if s.checking || s.installing() {
            return Err("Already checking.".into());
        }
        s.checking = true;
    }
    let r = (|| {
        let body = http_get(API_LATEST, "application/vnd.github+json", MAX_API)?;
        parse_release(&String::from_utf8_lossy(&body))
    })();
    let mut s = state();
    s.checking = false;
    s.checked = Some(Instant::now());
    match r {
        Ok(rel) => {
            s.error = None;
            let newer = is_newer(&rel.version, env!("CARGO_PKG_VERSION"));
            crate::log(&format!("update: the latest release is {} ({})", rel.version, if newer { "newer" } else { "this one or older" }));
            s.found = Some((rel.clone(), Instant::now()));
            Ok(Check { release: rel, newer })
        }
        Err(e) => {
            crate::log(&format!("update: check failed ({e})"));
            s.error = Some(e.clone());
            Err(e)
        }
    }
}

/// Le dossier ou l'exe telecharge est pose : `<dossier de l'application>\updates`.
pub fn updates_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("updates")
}

/// Le compte rendu de la derniere mise a jour (ecrit par `--self-update`, lu par la fenetre d'etat).
pub const LAST_NAME: &str = "last-update.json";

/// Telecharge SHA256SUMS de `rel` et sa signature, verifie la signature, PUIS telecharge l'exe, le verifie, l'ecrit
/// dans `dir` et le relit → son chemin et son SHA-256. Rien n'est lance.
pub fn fetch_verified(rel: &Release, dir: &Path) -> Result<(PathBuf, String), String> {
    fetch_verified_with(rel, dir, UPDATE_KEYS, &|url, max| http_get(url, "application/octet-stream", max))
}

/// `fetch_verified`, avec ses cles et son telechargement (`get` : l'adresse, la taille permise → les octets).
fn fetch_verified_with(rel: &Release, dir: &Path, keys: &[&str], get: &dyn Fn(&str, usize) -> Result<Vec<u8>, String>) -> Result<(PathBuf, String), String> {
    // (24 nov., audit E2) la signature d'abord : tant qu'elle n'est pas bonne, l'exe n'est ni telecharge ni ecrit
    let sums = get(&rel.sums_url(), MAX_SUMS)?;
    let sig = get(&rel.sig_url(), MAX_SIG)?;
    let key = verify_signature(&sums, &sig, &rel.tag, keys)?;
    let sums = std::str::from_utf8(&sums).map_err(|_| format!("{SUMS_NAME} of {} is not text: nothing is installed", rel.tag))?;
    let bytes = get(&rel.exe_url(), MAX_EXE)?;
    let sha = verify(&bytes, rel, sums)?;
    crate::log(&format!("update: {} downloaded, {SUMS_NAME} signed by update key {}, SHA-256 {sha} matches it", rel.version, key + 1));
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {} ({e})", dir.display()))?;
    let file = dir.join(format!("uppwind-companion-{}.exe", rel.version));
    let _ = std::fs::remove_file(&file);
    std::fs::write(&file, &bytes).map_err(|e| format!("cannot write {} ({e})", file.display()))?;
    // relu depuis le disque : ce qui est lance est ce qui a ete verifie
    let again = std::fs::read(&file).map_err(|e| format!("cannot read {} back ({e})", file.display()))?;
    if crate::plugin_install::sha256::hex(&again) != sha {
        let _ = std::fs::remove_file(&file);
        return Err("the file changed on disk after it was checked: nothing is installed".into());
    }
    // (11 nov.) la version que l'exe porte lui-meme (son bloc de version), pas seulement celle du tag
    if let Err(e) = own_version_is(&file, rel) {
        let _ = std::fs::remove_file(&file);
        return Err(e);
    }
    Ok((file, sha))
}

/// (11 nov.) Le bloc de version de `file` dit-il la version de la release ? (pas de bloc : non)
pub fn own_version_is(file: &Path, rel: &Release) -> Result<(), String> {
    version_matches(crate::wizard::firstrun::file_version(file).as_deref(), rel)
}

/// (pure) La version lue dans l'exe telecharge (None : illisible) est-elle celle de la release ?
pub fn version_matches(own: Option<&str>, rel: &Release) -> Result<(), String> {
    match own {
        Some(v) if parse_version(v).is_some() && parse_version(v) == parse_version(&rel.version) => Ok(()),
        Some(v) => Err(format!("the downloaded program says it is version {}, the release says {}: nothing is installed", v.chars().take(20).collect::<String>(), rel.version)),
        None => Err(format!("the downloaded program carries no version (the release says {}): nothing is installed", rel.version)),
    }
}

/// (11 nov.) Le compte rendu d'un `--self-update` (`updates\last-update.json`) : `version` celle qui s'installait,
/// `from` celle d'avant, `ok` tout s'est bien passe, `failed` elle n'a pas pris la place (installation ou lancement
/// rates : la copie d'avant est relancee), `errors` pourquoi, `done` ce qui a ete fait.
pub fn write_last(updates: &Path, version: &str, from: Option<&str>, ok: bool, failed: bool, errors: &[String], done: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(updates).map_err(|e| format!("cannot create {} ({e})", updates.display()))?;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let v = serde_json::json!({ "version": version, "from": from, "ok": ok, "failed": failed, "errors": errors, "done": done, "at": at });
    crate::state_file::write(&updates.join(LAST_NAME), &v).map_err(|e| format!("cannot write {} ({e})", updates.join(LAST_NAME).display()))
}

/// (pure) La mise a jour automatique passe-t-elle `version` ? Oui si le dernier compte rendu dit qu'elle a echoue
/// (jusqu'a ce qu'une version plus recente paraisse : elle a un autre numero). Un clic sur « Update » essaie quand meme.
pub fn auto_skips(last: Option<&Value>, version: &str) -> bool {
    last.is_some_and(|v| v["failed"] == Value::Bool(true) && v["version"].as_str().and_then(parse_version).is_some_and(|f| parse_version(version) == Some(f)))
}

/// `auto_skips` sur le compte rendu de `updates`.
pub fn auto_skips_in(updates: &Path, version: &str) -> bool {
    auto_skips(crate::state_file::read(&updates.join(LAST_NAME)).as_ref(), version)
}

/// (11 nov.) L'erreur de `download_and_start` quand un studio s'est connecte pendant le telechargement (la mise a
/// jour automatique) : l'icone reessaie plus tard (`AUTO_RETRY`).
pub const WAIT_BUSY: &str = "A studio is connected: the automatic update waits until it is done.";

/// `--check-update [--download <dossier>]` (la console) : lit la derniere release ; avec --download, la telecharge et
/// la verifie dans ce dossier, sans rien installer ni lancer. → le code de sortie (0 : a jour ou verifiee).
pub fn console(download: Option<&Path>) -> i32 {
    // (11 nov., relecture du Mac) ailleurs que sous Windows rien n'est demande a GitHub : on le dit tout de suite, avec
    // la page des versions, au lieu d'annoncer une question qui ne part pas
    if !cfg!(windows) {
        let _ = download;
        eprintln!("Uppwind Link {} · updates from the app are for Windows. On this computer nothing is checked or downloaded: get the new version from its release page, {RELEASES_PAGE}", env!("CARGO_PKG_VERSION"));
        return 1;
    }
    println!("Uppwind Link {} · asking GitHub for the latest release ({API_LATEST})", env!("CARGO_PKG_VERSION"));
    let c = match check_now() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not check: {e}");
            return 1;
        }
    };
    println!("latest release: {} ({}){}", c.release.version, c.release.tag, if c.newer { " · newer than this one" } else { " · not newer than this one" });
    let Some(dir) = download else { return 0 };
    match fetch_verified(&c.release, dir) {
        Ok((file, sha)) => {
            println!("downloaded and checked: {} ({SUMS_NAME} signed for {} by an Uppwind update key; SHA-256 {sha}, the same as {SUMS_NAME}{})", file.display(), c.release.tag, if c.release.exe_sha256.is_some() { " and GitHub's digest" } else { "" });
            println!("nothing was installed or started");
            0
        }
        Err(e) => {
            eprintln!("not verified: {e}");
            1
        }
    }
}

/// (24 nov., audit E2) `--verify-release <dossier> --release-tag v<x.y.z>` : sans rien telecharger, verifie les fichiers
/// d'une release poses dans `dossier` comme le ferait la mise a jour de CETTE version (ses cles) : la signature
/// SHA256SUMS.minisig de SHA256SUMS pour ce tag, puis le SHA-256 de l'exe et du .dmg presents, et (Windows) la
/// version de l'exe. Pour Lucas avant de publier (RELEASING.md), avec la version deja publiee. → le code de sortie.
pub fn verify_release_console(dir: &Path, tag: &str) -> i32 {
    match verify_release_dir(dir, tag, UPDATE_KEYS) {
        Ok(lines) => {
            for l in lines {
                println!("{l}");
            }
            println!("ok: Uppwind Link {} would accept {tag}", env!("CARGO_PKG_VERSION"));
            0
        }
        Err(e) => {
            eprintln!("NOT verified: {e}");
            1
        }
    }
}

/// `verify_release_console`, avec ses cles → ce qui a ete verifie, une ligne par fichier.
pub fn verify_release_dir(dir: &Path, tag: &str, keys: &[&str]) -> Result<Vec<String>, String> {
    if parse_version(tag).is_none() || !tag.starts_with('v') {
        return Err(format!("{} is not a release tag (v<x>.<y>.<z>)", clip(tag)));
    }
    let read = |name: &str, max: usize| -> Result<Vec<u8>, String> {
        let p = dir.join(name);
        let len = std::fs::metadata(&p).map_err(|e| format!("cannot read {} ({e})", p.display()))?.len();
        if len > max as u64 {
            return Err(format!("{} is larger than expected", p.display()));
        }
        std::fs::read(&p).map_err(|e| format!("cannot read {} ({e})", p.display()))
    };
    let sums = read(SUMS_NAME, MAX_SUMS)?;
    let sig = read(SIG_NAME, MAX_SIG)?;
    let key = verify_signature(&sums, &sig, tag, keys)?;
    let sums = std::str::from_utf8(&sums).map_err(|_| format!("{SUMS_NAME} is not text"))?;
    let mut out = vec![format!("{SIG_NAME}: a good signature of {SUMS_NAME} for {tag}, by update key {} of this version", key + 1)];
    let rel = Release { tag: tag.to_string(), version: tag[1..].to_string(), exe_size: 0, exe_sha256: None, urls: Default::default() };
    for name in [EXE_NAME, DMG_NAME] {
        let p = dir.join(name);
        if !p.is_file() {
            continue;
        }
        let want = sum_for(sums, name).ok_or_else(|| format!("{SUMS_NAME} has no line for {name}"))?;
        let bytes = read(name, MAX_EXE * 4)?;
        let got = crate::plugin_install::sha256::hex(&bytes);
        if got != want {
            return Err(format!("the SHA-256 of {name} ({}…) is not the one of {SUMS_NAME} ({}…)", &got[..12], &want[..12]));
        }
        out.push(format!("{name}: SHA-256 {got}, the same as {SUMS_NAME}"));
        if name == EXE_NAME {
            if !bytes.starts_with(b"MZ") {
                return Err(format!("{name} is not a Windows program"));
            }
            if cfg!(windows) {
                own_version_is(&p, &rel)?;
                out.push(format!("{name}: its version block says {}", rel.version));
            } else {
                out.push(format!("{name}: its version block is read on Windows only (not checked here)"));
            }
        }
    }
    if out.len() == 1 {
        return Err(format!("neither {EXE_NAME} nor {DMG_NAME} is in {}", dir.display()));
    }
    Ok(out)
}

/// (24 nov., audit E2) `--update-keys` : les cles de signature que cette version accepte. → 0 s'il y en a une ou deux,
/// bien formees et differentes (release.yml refuse de publier sinon : une version sans cle ne pourrait plus se mettre
/// a jour).
pub fn update_keys_console() -> i32 {
    match check_keys(UPDATE_KEYS) {
        Ok(()) => {
            for (i, k) in UPDATE_KEYS.iter().enumerate() {
                println!("update key {}: {k}", i + 1);
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Une liste de cles est-elle publiable ? (une ou deux, chacune une cle publique minisign Ed25519, toutes differentes)
/// (les caracteres 3 a 12 du base64 ne viennent que des octets 2 a 9, l'identifiant de la cle : deux cles au meme
/// identifiant se ressemblent la)
pub fn check_keys(keys: &[&str]) -> Result<(), String> {
    if keys.is_empty() {
        return Err("this build knows no update key (update::UPDATE_KEYS is empty): it would refuse every update".into());
    }
    if keys.len() > 2 {
        return Err("more than two update keys".into());
    }
    for (i, k) in keys.iter().enumerate() {
        minisign_verify::PublicKey::from_base64(k).map_err(|_| format!("update key {} is not a minisign public key", i + 1))?;
        if keys[..i].contains(k) || keys[..i].iter().any(|o| o.get(3..13) == k.get(3..13)) {
            return Err(format!("update key {} repeats another one (or its key ID)", i + 1));
        }
    }
    Ok(())
}

/// Telecharge la version trouvee par la derniere verification, la verifie, et la lance avec `--self-update`, qui
/// remplace ce compagnon. `show` : la fenetre d'etat de la nouvelle version s'ouvre a la fin (un clic sur « Update »).
/// `extra_args` : ceux du bac a sable. `busy` (la mise a jour automatique seulement) : relu juste avant de lancer le
/// nouvel exe ; true (un studio est connecte) → rien n'est lance, l'erreur `WAIT_BUSY`. → une ligne pour la page.
pub fn download_and_start(updates: &Path, show: bool, extra_args: &[String], busy: Option<&dyn Fn() -> bool>) -> Result<String, String> {
    let rel = {
        let mut s = state();
        if s.installing() {
            return Err("The update is already on its way.".into());
        }
        let Some((r, _)) = s.found.clone() else { return Err("Check for updates first.".into()) };
        if !is_newer(&r.version, env!("CARGO_PKG_VERSION")) {
            return Err("This is already the latest version.".into());
        }
        s.installing = Some(Instant::now());
        r
    };
    let r = (|| -> Result<String, String> {
        if !cfg!(windows) {
            return Err("Download the new version from its release page.".into());
        }
        let (file, _) = fetch_verified(&rel, updates)?;
        // (11 nov.) le telechargement a pu durer : un studio connecte entre-temps serait coupe
        if busy.is_some_and(|b| b()) {
            return Err(WAIT_BUSY.into());
        }
        let mut args = vec![SELF_UPDATE_FLAG.to_string()];
        if show {
            args.push("--show".into());
        }
        args.extend(extra_args.iter().cloned());
        crate::wizard::spawn_detached(&file, &args)?;
        Ok(format!("Installing Uppwind Link {}: it restarts in a few seconds.", rel.version))
    })();
    if r.is_err() {
        state().installing = None;
    }
    if let Err(e) = &r {
        crate::log(&format!("update: not installed ({e})"));
    }
    r
}

// ── le client https : WinHTTP de Windows (aucune crate de plus, les certificats et le proxy du systeme) ─────────

/// GET `url` (API_LATEST, ou une adresse de fichier que sa reponse donne, verifiee par `asset_url`) → le corps, au plus `max` octets, si la reponse est 200 et finit sur un
/// hote permis.
#[cfg(windows)]
pub fn http_get(url: &str, accept: &str, max: usize) -> Result<Vec<u8>, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Networking::WinHttp::*;
    struct H(*mut core::ffi::c_void);
    impl Drop for H {
        fn drop(&mut self) {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
    if !allowed_url(url) {
        return Err("not a GitHub address".into());
    }
    let rest = &url["https://".len()..];
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let path = if path.is_empty() { "/" } else { path };
    // (les erreurs de WinHTTP, 12001-12192, n'ont pas de texte dans les messages du systeme : leur numero)
    fn why(e: &windows::core::Error) -> String {
        let code = (e.code().0 as u32) & 0xFFFF;
        let m = e.message();
        if m.trim().is_empty() {
            format!("error {code}")
        } else {
            format!("{} (error {code})", m.trim())
        }
    }
    let neterr = |what: &str| format!("could not reach GitHub ({what}: {})", why(&windows::core::Error::from_thread()));
    unsafe {
        let ua = HSTRING::from(format!("Uppwind-Link-Companion/{}", env!("CARGO_PKG_VERSION")));
        let s = WinHttpOpen(&ua, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if s.is_null() {
            return Err(neterr("open"));
        }
        let s = H(s);
        let _ = WinHttpSetTimeouts(s.0, 15_000, 15_000, 20_000, 60_000);
        // TLS 1.2 et 1.3 (1.3 seulement la ou Windows l'a) ; jamais https → http
        let both = (WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | 0x2000).to_le_bytes();
        if WinHttpSetOption(Some(s.0), WINHTTP_OPTION_SECURE_PROTOCOLS, Some(&both)).is_err() {
            let _ = WinHttpSetOption(Some(s.0), WINHTTP_OPTION_SECURE_PROTOCOLS, Some(&WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2.to_le_bytes()));
        }
        let _ = WinHttpSetOption(Some(s.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP.to_le_bytes()));
        let c = WinHttpConnect(s.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0);
        if c.is_null() {
            return Err(neterr("connect"));
        }
        let c = H(c);
        let r = WinHttpOpenRequest(c.0, &HSTRING::from("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE);
        if r.is_null() {
            return Err(neterr("request"));
        }
        let r = H(r);
        let mut headers = format!("Accept: {accept}\r\n");
        if host == "api.github.com" {
            headers.push_str("X-GitHub-Api-Version: 2022-11-28\r\n");
        }
        let hw: Vec<u16> = headers.encode_utf16().collect();
        WinHttpSendRequest(r.0, Some(&hw), None, 0, 0, 0).map_err(|e| format!("could not reach GitHub ({})", why(&e)))?;
        WinHttpReceiveResponse(r.0, std::ptr::null_mut()).map_err(|e| format!("no answer from GitHub ({})", why(&e)))?;
        let mut code: u32 = 0;
        let mut len = 4u32;
        WinHttpQueryHeaders(r.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut code as *mut u32 as *mut _), &mut len, std::ptr::null_mut())
            .map_err(|e| format!("GitHub's answer has no status ({})", why(&e)))?;
        // l'adresse finale, apres les redirections
        let mut ulen = 0u32;
        let _ = WinHttpQueryOption(r.0, WINHTTP_OPTION_URL, None, &mut ulen);
        let mut ubuf = vec![0u16; (ulen as usize / 2).max(1)];
        let mut ulen2 = (ubuf.len() * 2) as u32;
        let final_url = match WinHttpQueryOption(r.0, WINHTTP_OPTION_URL, Some(ubuf.as_mut_ptr() as *mut _), &mut ulen2) {
            Ok(()) => String::from_utf16_lossy(&ubuf[..(ulen2 as usize / 2)]).trim_end_matches('\0').to_string(),
            Err(_) => String::new(),
        };
        if !allowed_url(&final_url) {
            return Err(format!("GitHub sent the download elsewhere ({}): refused", final_url.split('/').take(3).collect::<Vec<_>>().join("/")));
        }
        match code {
            200 => {}
            404 => return Err("no release found on GitHub".into()),
            403 | 429 => return Err("GitHub refused for now (too many checks from this address): try again in an hour".into()),
            c => return Err(format!("GitHub answered {c}")),
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(r.0, &mut avail).map_err(|e| format!("the download stopped ({})", why(&e)))?;
            if avail == 0 {
                break;
            }
            if out.len() + avail as usize > max {
                return Err("the download is larger than expected: refused".into());
            }
            let start = out.len();
            out.resize(start + avail as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(r.0, out[start..].as_mut_ptr() as *mut _, avail, &mut read).map_err(|e| format!("the download stopped ({})", why(&e)))?;
            out.truncate(start + read as usize);
        }
        Ok(out)
    }
}

#[cfg(not(windows))]
pub fn http_get(_url: &str, _accept: &str, _max: usize) -> Result<Vec<u8>, String> {
    Err("Updates from the app are for Windows: download the new version from its release page.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (28 nov.) L'adresse qu'en donne GitHub (le compte et le depot de l'essai : n'importe lesquels).
    fn dl(tag: &str, name: &str) -> String {
        format!("https://github.com/someone/some-repo/releases/download/{tag}/{name}")
    }

    fn test_urls(tag: &str) -> Urls {
        Urls { exe: dl(tag, EXE_NAME), sums: dl(tag, SUMS_NAME), sig: dl(tag, SIG_NAME) }
    }

    fn api(tag: &str, digest: Option<&str>, with_sums: bool) -> String {
        let mut assets = vec![serde_json::json!({ "name": EXE_NAME, "size": 6, "digest": digest, "browser_download_url": dl(tag, EXE_NAME) })];
        if with_sums {
            assets.push(serde_json::json!({ "name": SUMS_NAME, "size": 182, "browser_download_url": dl(tag, SUMS_NAME) }));
            assets.push(serde_json::json!({ "name": SIG_NAME, "size": 300, "browser_download_url": dl(tag, SIG_NAME) }));
        }
        serde_json::json!({ "tag_name": tag, "draft": false, "prerelease": false, "assets": assets }).to_string()
    }

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(parse_version("v0.3.0"), Some((0, 3, 0)));
        assert_eq!(parse_version("0.10.2"), Some((0, 10, 2)));
        for bad in ["", "v", "0.3", "0.3.0.1", "0.3.0-beta", "0.3.x", " 0.3.0", "v0.3.0 "] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
        assert!(is_newer("0.4.0", "0.3.0") && is_newer("0.10.0", "0.9.9") && is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.3.0", "0.3.0") && !is_newer("0.2.9", "0.3.0") && !is_newer("junk", "0.3.0"));
    }

    #[test]
    fn the_release_is_read_from_githubs_answer() {
        let d = "sha256:".to_string() + &"Ab".repeat(32);
        let r = parse_release(&api("v0.4.0", Some(&d), true)).unwrap();
        assert_eq!(r, Release { tag: "v0.4.0".into(), version: "0.4.0".into(), exe_size: 6, exe_sha256: Some("ab".repeat(32)), urls: test_urls("v0.4.0") });
        // (28 nov.) les adresses sont celles de la reponse (demandee par le numero du depot), pas faites avec un nom
        assert_eq!(r.exe_url(), "https://github.com/someone/some-repo/releases/download/v0.4.0/uppwind-companion.exe");
        assert_eq!(r.sums_url(), "https://github.com/someone/some-repo/releases/download/v0.4.0/SHA256SUMS");
        assert_eq!(parse_release(&api("v0.4.0", None, true)).unwrap().exe_sha256, None);
        assert!(parse_release(&api("v0.4.0", None, false)).unwrap_err().contains("SHA256SUMS"), "no sums, no update");
        // (24 nov., audit E2) sans signature, pas de mise a jour
        let mut v: Value = serde_json::from_str(&api("v0.4.0", None, true)).unwrap();
        v["assets"].as_array_mut().unwrap().retain(|a| a["name"] != SIG_NAME);
        assert!(parse_release(&v.to_string()).unwrap_err().contains("not signed"));
        assert_eq!(parse_release(&api("v0.4.0", None, true)).unwrap().sig_url(), "https://github.com/someone/some-repo/releases/download/v0.4.0/SHA256SUMS.minisig");
        assert!(parse_release(&api("v0.4.0/../x", None, true)).is_err());
        assert!(parse_release(&api("0.4.0", None, true)).is_err(), "tags are v<x>.<y>.<z>");
        assert!(parse_release(&api("v0.4.0", Some("md5:abc"), true)).is_err());
        let mut v: Value = serde_json::from_str(&api("v0.4.0", None, true)).unwrap();
        v["prerelease"] = Value::Bool(true);
        assert!(parse_release(&v.to_string()).is_err());
        assert!(parse_release("<html>").is_err());
        assert!(parse_release(r#"{"tag_name":"v0.4.0","assets":[]}"#).is_err());
        // la vraie reponse de api.github.com/repositories/1434113356/releases/latest (28 nov., v0.4.15), reduite
        let real = r#"{"tag_name":"v0.4.15","draft":false,"prerelease":false,"assets":[
            {"name":"SHA256SUMS","size":182,"digest":"sha256:cf136637a4b5bb66d2a11d6e8b8e3bae86f124b28c7ca01b29787d63382cff69","browser_download_url":"https://github.com/kaonaya-studio/uppwind-app/releases/download/v0.4.15/SHA256SUMS"},
            {"name":"uppwind-companion-macos.dmg","size":13837559,"digest":"sha256:a019c1cd9c63a232b20a43f83c8b61e778c3d28d767f04e92f96fb579bf6843f","browser_download_url":"https://github.com/kaonaya-studio/uppwind-app/releases/download/v0.4.15/uppwind-companion-macos.dmg"},
            {"name":"uppwind-companion.exe","size":7607808,"digest":"sha256:7d205953abd32c21ce2af32b486c0695feccee08328829c7de5abb2eb30165f5","browser_download_url":"https://github.com/kaonaya-studio/uppwind-app/releases/download/v0.4.15/uppwind-companion.exe"}]}"#;
        assert!(parse_release(real).unwrap_err().contains("not signed"), "the releases before 0.4.16 are not signed");
        let signed = real.replace(
            r#"{"name":"uppwind-companion-macos.dmg""#,
            r#"{"name":"SHA256SUMS.minisig","size":300,"browser_download_url":"https://github.com/kaonaya-studio/uppwind-app/releases/download/v0.4.15/SHA256SUMS.minisig"},{"name":"uppwind-companion-macos.dmg""#,
        );
        let r = parse_release(&signed).unwrap();
        assert_eq!((r.version.as_str(), r.exe_size), ("0.4.15", 7607808));
        assert_eq!(r.exe_url(), "https://github.com/kaonaya-studio/uppwind-app/releases/download/v0.4.15/uppwind-companion.exe");
        assert!(!is_newer(&r.version, "0.4.15"));
    }

    #[test]
    fn download_addresses_come_from_the_answer_and_have_one_shape() {
        let with_url = |name: &str, url: Value| {
            let mut v: Value = serde_json::from_str(&api("v0.4.0", None, true)).unwrap();
            for a in v["assets"].as_array_mut().unwrap() {
                if a["name"] == name {
                    a["browser_download_url"] = url.clone();
                }
            }
            parse_release(&v.to_string())
        };
        // un autre compte, un autre nom de depot : accepte (le numero du depot a ete demande ; la signature decide)
        assert!(with_url(EXE_NAME, Value::from("https://github.com/x/y/releases/download/v0.4.0/uppwind-companion.exe")).is_ok());
        for name in [EXE_NAME, SUMS_NAME, SIG_NAME] {
            let other = if name == EXE_NAME { SUMS_NAME } else { EXE_NAME };
            for bad in [
                Value::Null,
                Value::from(""),
                Value::from(format!("http://github.com/a/b/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://evil.example/a/b/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://github.com.evil.example/a/b/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://objects.githubusercontent.com/a/b/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://user@github.com/a/b/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://github.com/a/b/releases/download/v0.3.0/{name}")),
                Value::from(format!("https://github.com/a/b/releases/download/v0.4.0/{other}")),
                Value::from(format!("https://github.com/a/b/releases/download/v0.4.0/{name}?x=1")),
                Value::from(format!("https://github.com/a/b/releases/download/v0.4.0/{name}#x")),
                Value::from(format!("https://github.com/a/b/releases/download/v0.4.0/x/{name}")),
                Value::from(format!("https://github.com/../b/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://github.com//b/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://github.com/a/b/releases/tag/v0.4.0/{name}")),
                Value::from(format!("https://github.com/a/b%2Fc/releases/download/v0.4.0/{name}")),
                Value::from(format!("https://github.com/{}/b/releases/download/v0.4.0/{name}", "a".repeat(300))),
            ] {
                let e = with_url(name, bad.clone()).unwrap_err();
                assert!(e.contains("download address"), "{name} {bad}: {e}");
            }
        }
    }

    #[test]
    fn no_account_name_is_used_to_ask_github() {
        // (28 nov.) l'API est demandee par le numero du depot ; aucun nom de compte, ni l'ancien ni l'actuel
        assert_eq!(API_LATEST, format!("https://api.github.com/repositories/{REPO_ID}/releases/latest"));
        assert!(allowed_url(API_LATEST));
        let old = ["alaka", "zam777"].concat();
        for s in [API_LATEST, RELEASES_PAGE, include_str!("update.rs"), include_str!("ui/web/app.js"), include_str!("../Cargo.toml")] {
            assert!(!s.contains(&old), "the old account name is still there");
        }
        assert!(!API_LATEST.contains("kaonaya-studio") && !API_LATEST.contains("/repos/"));
    }

    #[test]
    fn sha256sums_lines_are_matched_by_exact_name() {
        let a = "5820c3e89a6ec1ef4f053ea3bcdfac6c0d64f43b567a0b56fa75ed16180d721a";
        let b = "3967c707d15755e347d1504088b20a07b4d976c086c87752ec292d9704b66cd4";
        let real = format!("{a}  uppwind-companion.exe\n{b}  uppwind-companion-macos.dmg\n");
        assert_eq!(sum_for(&real, EXE_NAME).as_deref(), Some(a));
        assert_eq!(sum_for(&real, "uppwind-companion-macos.dmg").as_deref(), Some(b));
        assert_eq!(sum_for(&format!("{}  uppwind-companion.exe\r\n", a.to_uppercase()), EXE_NAME).as_deref(), Some(a), "CRLF, upper case");
        assert_eq!(sum_for(&format!("{a} *uppwind-companion.exe"), EXE_NAME).as_deref(), Some(a), "binary mode mark");
        assert_eq!(sum_for(&format!("{a}  uppwind-companion.exe.old"), EXE_NAME), None);
        assert_eq!(sum_for(&format!("{a}  x/uppwind-companion.exe"), EXE_NAME), None);
        assert_eq!(sum_for("abc  uppwind-companion.exe", EXE_NAME), None);
        assert_eq!(sum_for(&format!("{a}  uppwind-companion.exe\n{b}  uppwind-companion.exe"), EXE_NAME), None, "two answers: none");
    }

    #[test]
    fn the_download_must_match_the_release() {
        let bytes = b"MZ\x90\0ab".to_vec();
        // (le SHA-256 de ces six octets, calcule par hashlib de Python : pas par le code qu'on essaie)
        let h = "cf820ff9fd6418780b5eeee670a7ea2a3cc52acd6181d218ca8fe217dfb57930".to_string();
        assert_eq!(crate::plugin_install::sha256::hex(&bytes), h);
        let sums = format!("{h}  {EXE_NAME}\n");
        let rel = Release { tag: "v9.0.0".into(), version: "9.0.0".into(), exe_size: 6, exe_sha256: Some(h.clone()), urls: Default::default() };
        assert_eq!(verify(&bytes, &rel, &sums), Ok(h.clone()));
        assert!(verify(&bytes, &Release { exe_sha256: None, ..rel.clone() }, &sums).is_ok(), "SHA256SUMS alone is enough");
        let mut bad = bytes.clone();
        bad[5] ^= 1;
        assert!(verify(&bad, &rel, &sums).unwrap_err().contains("SHA-256"));
        assert!(verify(&bytes[..5], &rel, &sums).unwrap_err().contains("bytes"));
        assert!(verify(b"PK\x90\0ab", &rel, &sums).unwrap_err().contains("Windows program"));
        assert!(verify(&bytes, &Release { exe_sha256: Some("0".repeat(64)), ..rel.clone() }, &sums).unwrap_err().contains("disagree"));
        assert!(verify(&bytes, &rel, "").unwrap_err().contains("no line"));
    }

    #[test]
    fn only_github_addresses_are_allowed() {
        for ok in [API_LATEST, "https://github.com/x", "https://release-assets.githubusercontent.com/github-production-release-asset/1?sig=a", "https://objects.githubusercontent.com/a"] {
            assert!(allowed_url(ok), "{ok}");
        }
        for bad in ["http://github.com/x", "https://github.com.evil.example/x", "https://evil.example/github.com", "https://user@github.com/x", "https://github.com:444/x", "", "https://beta.uppwind.com/"] {
            assert!(!allowed_url(bad), "{bad}");
        }
        assert!(RELEASES_PAGE.starts_with("https://github.com/") && RELEASES_PAGE.ends_with("/releases/latest"));
    }

    #[test]
    fn the_automatic_choice_is_kept_in_the_state_file_and_off_by_default() {
        let d = std::env::temp_dir().join(format!("uwc-upd-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("state.json");
        std::fs::write(&f, r#"{"token":"t"}"#).unwrap();
        assert!(!auto_on(&f), "off until ticked");
        // (24 nov., audit E2) retiree tant que les mises a jour ne sont pas signees : ni cochee, ni lue
        if !AUTO_AVAILABLE {
            assert!(set_auto(&f, true).is_err());
            std::fs::write(&f, r#"{"token":"t","autoUpdate":true}"#).unwrap();
            assert!(auto_ticked(&f) && !auto_on(&f), "ticked by an older version: it does nothing");
            std::fs::write(&f, r#"{"token":"t"}"#).unwrap();
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        assert_eq!(set_auto(&f, true), Ok(true));
        assert!(auto_on(&f));
        assert_eq!(crate::state_file::read(&f).unwrap()["token"], "t", "the rest of the file stays");
        set_auto(&f, false).unwrap();
        assert!(!auto_on(&f));
        assert!(!auto_on(&d.join("missing.json")));
        assert!(set_auto(&d.join("missing.json"), true).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_downloaded_program_must_carry_the_releases_version() {
        let rel = Release { tag: "v0.5.0".into(), version: "0.5.0".into(), exe_size: 6, exe_sha256: None, urls: Default::default() };
        assert_eq!(version_matches(Some("0.5.0"), &rel), Ok(()));
        let e = version_matches(Some("0.4.0"), &rel).unwrap_err();
        assert!(e.contains("says it is version 0.4.0") && e.contains("the release says 0.5.0"), "{e}");
        assert!(version_matches(Some("0.6.0"), &rel).is_err());
        assert!(version_matches(Some("junk"), &rel).is_err());
        assert!(version_matches(None, &rel).unwrap_err().contains("carries no version"));
        // un vrai fichier : sans bloc de version, refuse ; celui de ping.exe (10.x) n'est pas 0.5.0
        let d = std::env::temp_dir().join(format!("uwc-upd-ver-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("uppwind-companion-0.5.0.exe");
        std::fs::write(&f, b"MZ not really").unwrap();
        assert!(own_version_is(&f, &rel).is_err());
        #[cfg(windows)]
        {
            let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("PING.EXE");
            // (comme firstrun : 10.x, ou 6.x selon Windows)
            let e = own_version_is(&ping, &rel).unwrap_err();
            assert!(e.contains("says it is version 10.") || e.contains("says it is version 6."), "{e}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_version_is_recorded_and_skipped_by_the_automatic_update_only() {
        let d = std::env::temp_dir().join(format!("uwc-upd-last-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(!auto_skips_in(&d, "0.5.0"), "no report yet");
        write_last(&d, "0.5.0", Some("0.4.0"), false, true, &["cannot copy to x (disk full)".into()], &[]).unwrap();
        let v = crate::state_file::read(&d.join(LAST_NAME)).unwrap();
        assert_eq!((v["version"].as_str(), v["from"].as_str(), v["ok"].as_bool(), v["failed"].as_bool()), (Some("0.5.0"), Some("0.4.0"), Some(false), Some(true)));
        assert_eq!(v["errors"][0], "cannot copy to x (disk full)");
        assert!(auto_skips_in(&d, "0.5.0"), "the failed version waits for a click");
        assert!(!auto_skips_in(&d, "0.5.1"), "a newer one is tried");
        // la fenetre d'etat le lit toujours
        assert_eq!(view(None, Some(&d)).last.unwrap()["version"], "0.5.0");
        // installee, meme avec un souci : rien n'est passe
        write_last(&d, "0.5.0", Some("0.4.0"), false, false, &["FL Studio: x".into()], &[]).unwrap();
        assert!(!auto_skips_in(&d, "0.5.0"));
        write_last(&d, "0.5.0", None, true, false, &[], &["Copied".into()]).unwrap();
        assert!(!auto_skips_in(&d, "0.5.0"));
        assert!(!auto_skips(Some(&serde_json::json!({ "version": "junk", "failed": true })), "junk"));
        // un clic sur « Update » ne lit pas ce compte rendu : download_and_start n'en depend pas
        let _ = std::fs::remove_dir_all(&d);
    }

    // ── (24 nov., audit E2) la signature de SHA256SUMS ─────────────────────────────────────────────────────────
    // Des signatures d'essai au format minisign, faites par un script Python (cryptography : Ed25519 ; hashlib :
    // BLAKE2b-512), pas par le code essaye ici. Les cles privees n'ont vecu que dans ce script : seules les cles
    // publiques et les signatures sont ici. A et B : deux cles d'essai ; SUMS : le SHA256SUMS signe (l'exe des six
    // octets de the_download_must_match_the_release), pour le tag v9.0.0 sauf SIG_OTHER_TAG (v8.0.0) ; SIG_LEGACY :
    // l'ancien mode de minisign (non pre-hache) ; SIG_FORGED_KEYID : signee par B, avec l'identifiant de cle de A.
    const SUMS: &str = "cf820ff9fd6418780b5eeee670a7ea2a3cc52acd6181d218ca8fe217dfb57930  uppwind-companion.exe\n";
    const TEST_PUB_A: &str = "RWQzKfgSr+++bIo/2zulEittDtoFGLaXydp88MPMa1qpwsatEnjoNDbM";
    const TEST_PUB_B: &str = "RWSoPDlESWZ0YB74rCzaRK1f9GY0X6WEua0c+y65YWP50tJ7UjORnYph";
    const SIG_OK: &str = "untrusted comment: test signature (Python), not a real key\nRUQzKfgSr+++bAwM/TVZkJOC/MeX5OZk5biuSy/QPvjSwmFZYsD0CkjM8kQicloFqsZjABCS+9VEHbFtw/XIworuV0xkPXC9Rwo=\ntrusted comment: uppwind-companion v9.0.0\nHVFoIUd5m6Q2DrEmoenct1TVntUAr/4oKoTXG2AgynpCYLbT8XGNqISnaH78a61leLcrUhfajf+qayGk5LswDA==\n";
    const SIG_OTHER_TAG: &str = "untrusted comment: test signature (Python), not a real key\nRUQzKfgSr+++bAwM/TVZkJOC/MeX5OZk5biuSy/QPvjSwmFZYsD0CkjM8kQicloFqsZjABCS+9VEHbFtw/XIworuV0xkPXC9Rwo=\ntrusted comment: uppwind-companion v8.0.0\n4sSN0B9VGjhqlu41ACW9fQ3R/qe4qOK8E/sx7H381zjNG/grYf3TgcVwgYyIeCtrj4IiklVO16ZS2/mg57S8Cw==\n";
    const SIG_KEY_B: &str = "untrusted comment: test signature (Python), not a real key\nRUSoPDlESWZ0YORNmtaJ/ueiXPdUuGnGIr3zoNT+ztzvB2z2dGZcfeAL6036h16gKR4PH4YhM5HmCQTfQhyOS+QA+Wi1XFpeJAY=\ntrusted comment: uppwind-companion v9.0.0\nGl5RG4RmOhAcl4qG8PyVT7UI7EuJqw4ek8YR5rSlEz3sRp82jsbCpyCkVbJSlnfxrcXd7un2XGuwIK+dSKkJBw==\n";
    const SIG_LEGACY: &str = "untrusted comment: test signature (Python), not a real key\nRWQzKfgSr+++bNBgh7RLp/yDix51/dau+HlckPuTIvm1MOTRLHKuxEwPgHpW7mij5CBj/q87mTNqpKZ468pcqGO+k40mKB5e/Qw=\ntrusted comment: uppwind-companion v9.0.0\nArJMXSLQwB6k9aQfQqJZUoCUUlybyv5tJrXtH2cZV9HH8pQwNaxqpC6Vup2i/E+oqWBDdE2hUZXX/91KrzSwAQ==\n";
    const SIG_FORGED_KEYID: &str = "untrusted comment: test signature (Python), not a real key\nRUQzKfgSr+++bORNmtaJ/ueiXPdUuGnGIr3zoNT+ztzvB2z2dGZcfeAL6036h16gKR4PH4YhM5HmCQTfQhyOS+QA+Wi1XFpeJAY=\ntrusted comment: uppwind-companion v9.0.0\nGl5RG4RmOhAcl4qG8PyVT7UI7EuJqw4ek8YR5rSlEz3sRp82jsbCpyCkVbJSlnfxrcXd7un2XGuwIK+dSKkJBw==\n";

    #[test]
    fn a_good_signature_is_accepted_by_either_key() {
        assert_eq!(verify_signature(SUMS.as_bytes(), SIG_OK.as_bytes(), "v9.0.0", &[TEST_PUB_A]), Ok(0));
        // deux cles (la principale, celle de secours) : chacune est acceptee, a sa place dans la liste
        assert_eq!(verify_signature(SUMS.as_bytes(), SIG_OK.as_bytes(), "v9.0.0", &[TEST_PUB_B, TEST_PUB_A]), Ok(1));
        assert_eq!(verify_signature(SUMS.as_bytes(), SIG_KEY_B.as_bytes(), "v9.0.0", &[TEST_PUB_A, TEST_PUB_B]), Ok(1));
        // CRLF (un .minisig passe par un editeur) : les memes lignes
        assert_eq!(verify_signature(SUMS.as_bytes(), SIG_OK.replace('\n', "\r\n").as_bytes(), "v9.0.0", &[TEST_PUB_A]), Ok(0));
    }

    #[test]
    fn no_key_refuses_everything() {
        let e = verify_signature(SUMS.as_bytes(), SIG_OK.as_bytes(), "v9.0.0", &[]).unwrap_err();
        assert!(e.contains("knows no signing key"), "{e}");
    }

    #[test]
    fn a_missing_or_malformed_signature_is_refused() {
        let k = &[TEST_PUB_A];
        let mut cut = SIG_OK.lines().collect::<Vec<_>>();
        cut.truncate(3);
        let bad_b64 = SIG_OK.replacen("RUQz", "R!Qz", 1);
        let short = SIG_OK.replacen("RUQzKfgSr+++b", "RUQzKfgSr+b", 1);
        let no_tc = SIG_OK.replace("trusted comment: ", "comment: ");
        for sig in ["", "\n\n\n\n", "<html>404</html>", &cut.join("\n"), &bad_b64, &short, &no_tc] {
            let e = verify_signature(SUMS.as_bytes(), sig.as_bytes(), "v9.0.0", k).unwrap_err();
            assert!(e.contains("malformed"), "{sig:?}: {e}");
        }
        assert!(verify_signature(SUMS.as_bytes(), &[0xff, 0xfe, 0x00], "v9.0.0", k).unwrap_err().contains("malformed"), "not UTF-8");
        // une autre sorte de signature (ni « Ed » ni « ED »)
        let other_alg = SIG_OK.replacen("RUQz", "RUUz", 1);
        assert!(verify_signature(SUMS.as_bytes(), other_alg.as_bytes(), "v9.0.0", k).is_err());
    }

    #[test]
    fn a_false_signature_is_refused() {
        let k = &[TEST_PUB_A];
        // SHA256SUMS change d'un octet (un autre exe)
        let other = SUMS.replacen("cf82", "cf83", 1);
        assert!(verify_signature(other.as_bytes(), SIG_OK.as_bytes(), "v9.0.0", k).unwrap_err().contains("is false"));
        // une ligne ajoutee (un 2e fichier)
        let more = format!("{SUMS}{}  {DMG_NAME}\n", "0".repeat(64));
        assert!(verify_signature(more.as_bytes(), SIG_OK.as_bytes(), "v9.0.0", k).unwrap_err().contains("is false"));
        // le commentaire signe change apres coup (pour faire passer la signature de v8 pour v9) : la signature
        // globale ne tient plus
        let moved = SIG_OTHER_TAG.replace("uppwind-companion v8.0.0", "uppwind-companion v9.0.0");
        assert!(verify_signature(SUMS.as_bytes(), moved.as_bytes(), "v9.0.0", k).unwrap_err().contains("is false"));
        // l'identifiant de la cle A sur une signature de B
        assert!(verify_signature(SUMS.as_bytes(), SIG_FORGED_KEYID.as_bytes(), "v9.0.0", k).unwrap_err().contains("is false"));
        // un octet de la signature change
        let mut lines: Vec<String> = SIG_OK.lines().map(String::from).collect();
        lines[1] = lines[1].replacen("AwM", "AwN", 1);
        assert!(verify_signature(SUMS.as_bytes(), lines.join("\n").as_bytes(), "v9.0.0", k).unwrap_err().contains("is false"));
        let mut lines: Vec<String> = SIG_OK.lines().map(String::from).collect();
        lines[3] = lines[3].replacen("HVF", "HVG", 1);
        assert!(verify_signature(SUMS.as_bytes(), lines.join("\n").as_bytes(), "v9.0.0", k).unwrap_err().contains("is false"));
    }

    #[test]
    fn an_unknown_key_or_the_old_mode_is_refused() {
        let e = verify_signature(SUMS.as_bytes(), SIG_KEY_B.as_bytes(), "v9.0.0", &[TEST_PUB_A]).unwrap_err();
        assert!(e.contains("does not know"), "{e}");
        let e = verify_signature(SUMS.as_bytes(), SIG_LEGACY.as_bytes(), "v9.0.0", &[TEST_PUB_A]).unwrap_err();
        assert!(e.contains("old mode"), "{e}");
        assert!(verify_signature(SUMS.as_bytes(), SIG_OK.as_bytes(), "v9.0.0", &["RWQjunk"]).unwrap_err().contains("key 1 of this version is malformed"));
    }

    #[test]
    fn a_signature_of_another_release_is_refused() {
        // la signature (vraie) de v8.0.0 resservie sous le tag v9.0.0, et l'inverse
        let e = verify_signature(SUMS.as_bytes(), SIG_OTHER_TAG.as_bytes(), "v9.0.0", &[TEST_PUB_A]).unwrap_err();
        assert!(e.contains("made for another release") && e.contains("uppwind-companion v8.0.0"), "{e}");
        assert!(verify_signature(SUMS.as_bytes(), SIG_OK.as_bytes(), "v9.0.1", &[TEST_PUB_A]).unwrap_err().contains("another release"));
        assert!(verify_signature(SUMS.as_bytes(), SIG_OK.as_bytes(), "9.0.0", &[TEST_PUB_A]).is_err());
        assert_eq!(signed_comment("v0.4.16"), "uppwind-companion v0.4.16");
    }

    /// Un faux GitHub : les fichiers d'une release, et la liste des adresses demandees.
    fn fake_get<'a>(files: Vec<(&'a str, Vec<u8>)>, asked: &'a std::cell::RefCell<Vec<String>>) -> impl Fn(&str, usize) -> Result<Vec<u8>, String> + 'a {
        move |url: &str, max: usize| {
            asked.borrow_mut().push(url.rsplit('/').next().unwrap_or("").to_string());
            let name = url.rsplit('/').next().unwrap_or("");
            let b = files.iter().find(|(n, _)| *n == name).map(|(_, b)| b.clone()).ok_or("no release found on GitHub")?;
            if b.len() > max {
                return Err("the download is larger than expected: refused".into());
            }
            Ok(b)
        }
    }

    #[test]
    fn without_a_good_signature_the_exe_is_neither_downloaded_nor_written() {
        let exe = b"MZ\x90\0ab".to_vec();
        let rel = Release { tag: "v9.0.0".into(), version: "9.0.0".into(), exe_size: 6, exe_sha256: None, urls: test_urls("v9.0.0") };
        let d = std::env::temp_dir().join(format!("uwc-upd-sig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let cases: Vec<(&str, Option<&str>, &[&str])> = vec![
            ("no signature file", None, &[TEST_PUB_A]),
            ("malformed", Some("junk"), &[TEST_PUB_A]),
            ("unknown key", Some(SIG_KEY_B), &[TEST_PUB_A]),
            ("another release", Some(SIG_OTHER_TAG), &[TEST_PUB_A]),
            ("no key in this version", Some(SIG_OK), &[]),
        ];
        for (what, sig, keys) in cases {
            let asked = std::cell::RefCell::new(Vec::new());
            let mut files = vec![(SUMS_NAME, SUMS.as_bytes().to_vec()), (EXE_NAME, exe.clone())];
            if let Some(s) = sig {
                files.push((SIG_NAME, s.as_bytes().to_vec()));
            }
            let get = fake_get(files, &asked);
            assert!(fetch_verified_with(&rel, &d, keys, &get).is_err(), "{what}");
            assert!(!asked.borrow().iter().any(|n| n == EXE_NAME), "{what}: the exe was downloaded");
            assert!(!d.exists(), "{what}: something was written");
        }
        // signee : l'exe est telecharge, verifie, ecrit ; ces six octets n'ont pas de bloc de version, donc il est
        // refuse a la derniere etape et efface
        let asked = std::cell::RefCell::new(Vec::new());
        let get = fake_get(vec![(SUMS_NAME, SUMS.as_bytes().to_vec()), (SIG_NAME, SIG_OK.as_bytes().to_vec()), (EXE_NAME, exe.clone())], &asked);
        let e = fetch_verified_with(&rel, &d, &[TEST_PUB_A], &get).unwrap_err();
        assert!(e.contains("carries no version"), "{e}");
        assert_eq!(*asked.borrow(), vec![SUMS_NAME.to_string(), SIG_NAME.to_string(), EXE_NAME.to_string()], "the signature before the exe");
        assert!(!d.join("uppwind-companion-9.0.0.exe").exists());
        // un exe change (le SHA256SUMS signe ne le couvre pas)
        let asked = std::cell::RefCell::new(Vec::new());
        let get = fake_get(vec![(SUMS_NAME, SUMS.as_bytes().to_vec()), (SIG_NAME, SIG_OK.as_bytes().to_vec()), (EXE_NAME, b"MZ\x90\0ac".to_vec())], &asked);
        assert!(fetch_verified_with(&rel, &d, &[TEST_PUB_A], &get).unwrap_err().contains("SHA-256"));
        assert!(!d.join("uppwind-companion-9.0.0.exe").exists());
        // une signature trop grosse n'est pas lue
        let asked = std::cell::RefCell::new(Vec::new());
        let get = fake_get(vec![(SUMS_NAME, SUMS.as_bytes().to_vec()), (SIG_NAME, vec![b'a'; MAX_SIG + 1]), (EXE_NAME, exe)], &asked);
        assert!(fetch_verified_with(&rel, &d, &[TEST_PUB_A], &get).unwrap_err().contains("larger"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn verify_release_checks_a_folder_offline() {
        let d = std::env::temp_dir().join(format!("uwc-upd-vr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(SUMS_NAME), SUMS).unwrap();
        std::fs::write(d.join(EXE_NAME), b"MZ\x90\0ab").unwrap();
        assert!(verify_release_dir(&d, "v9.0.0", &[TEST_PUB_A]).unwrap_err().contains("cannot read"), "no signature file");
        std::fs::write(d.join(SIG_NAME), SIG_OK).unwrap();
        assert!(verify_release_dir(&d, "v9.0.1", &[TEST_PUB_A]).unwrap_err().contains("another release"));
        assert!(verify_release_dir(&d, "v9.0.0", &[TEST_PUB_B]).unwrap_err().contains("does not know"));
        assert!(verify_release_dir(&d, "v9.0.0", &[]).unwrap_err().contains("knows no signing key"));
        assert!(verify_release_dir(&d, "../x", &[TEST_PUB_A]).is_err());
        let r = verify_release_dir(&d, "v9.0.0", &[TEST_PUB_A]);
        if cfg!(windows) {
            // signature et SHA-256 bons ; ces six octets n'ont pas de bloc de version
            assert!(r.unwrap_err().contains("carries no version"));
        } else {
            let lines = r.unwrap();
            assert!(lines[0].contains("good signature") && lines[1].contains("SHA-256 cf820ff9"), "{lines:?}");
        }
        std::fs::write(d.join(EXE_NAME), b"MZ\x90\0ac").unwrap();
        assert!(verify_release_dir(&d, "v9.0.0", &[TEST_PUB_A]).unwrap_err().contains("is not the one of"));
        std::fs::remove_file(d.join(EXE_NAME)).unwrap();
        assert!(verify_release_dir(&d, "v9.0.0", &[TEST_PUB_A]).unwrap_err().contains("neither"));
        // un .dmg absent de SHA256SUMS
        std::fs::write(d.join(DMG_NAME), b"x").unwrap();
        assert!(verify_release_dir(&d, "v9.0.0", &[TEST_PUB_A]).unwrap_err().contains("no line for"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_production_keys_are_publishable_and_never_the_test_keys() {
        assert!(!UPDATE_KEYS.contains(&TEST_PUB_A) && !UPDATE_KEYS.contains(&TEST_PUB_B), "a test key in UPDATE_KEYS");
        assert_eq!(AUTO_AVAILABLE, !UPDATE_KEYS.is_empty());
        if !UPDATE_KEYS.is_empty() {
            assert_eq!(check_keys(UPDATE_KEYS), Ok(()));
        }
        assert!(check_keys(&[]).unwrap_err().contains("no update key"));
        assert_eq!(check_keys(&[TEST_PUB_A, TEST_PUB_B]), Ok(()));
        assert!(check_keys(&[TEST_PUB_A, TEST_PUB_A]).is_err());
        assert!(check_keys(&[TEST_PUB_A, TEST_PUB_B, TEST_PUB_A]).is_err());
        assert!(check_keys(&["RWQnot-a-key"]).is_err());
        // la meme cle d'identifiant que A, une autre cle : repere par l'identifiant
        let same_id = format!("{}{}", &TEST_PUB_A[..14], &TEST_PUB_B[14..]);
        assert!(check_keys(&[TEST_PUB_A, &same_id]).is_err());
    }

    #[test]
    fn nothing_downloads_without_a_newer_release() {
        let d = std::env::temp_dir();
        assert!(download_and_start(&d, false, &[], None).is_err());
        assert!(!view(None, None).auto, "no state file: off");
    }
}

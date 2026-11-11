// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! (11 nov.) La mise a jour du compagnon, depuis sa fenetre d'etat : la seule connexion du compagnon a internet.
//!
//! - Une seule destination : les releases du depot public GitHub (`api.github.com/repos/alakazam777/uppwind-app/
//!   releases/latest`, puis `github.com/alakazam777/uppwind-app/releases/download/<tag>/…`, que GitHub redirige vers ses
//!   serveurs de fichiers). Les adresses sont des constantes de l'exe ; l'adresse finale de chaque reponse (apres les
//!   redirections) doit etre sur un hote GitHub de la liste `HOSTS`, en https (WinHTTP refuse https → http).
//! - Rien n'est envoye : des GET sans corps, sans cookie, sans jeton ; les en-tetes poses ici : User-Agent
//!   « Uppwind-Link-Companion/<version> » (GitHub l'exige), Accept, et pour l'API X-GitHub-Api-Version (WinHTTP ajoute
//!   les siens, Host...). GitHub voit l'adresse IP, comme pour tout telechargement.
//! - Jamais sans un clic : « Check for updates », ou la case « Update automatically » (off par defaut, gardee dans le
//!   fichier d'etat, « autoUpdate »), cochee dans la fenetre d'etat seulement (jamais par le studio : ipc.rs).
//! - Le nouvel exe n'est pas signe (RELEASING.md, etape 6) : il est verifie par son SHA-256, celui du fichier
//!   `SHA256SUMS` de la meme release, ET celui que l'API de GitHub donne pour le fichier (`digest`) quand elle le donne,
//!   qui doivent etre egaux ; sa taille, celle de l'API ; ses deux premiers octets « MZ » ; (11 nov.) la version de son
//!   bloc de version, celle du tag. Cela prouve que l'exe est celui que la
//!   release publie, intact. Cela ne prouve pas qui l'a publie : quelqu'un qui tiendrait le compte GitHub pourrait
//!   publier un autre exe et son SHA256SUMS (voir le rapport et README, « Updates »).
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

#[cfg(test)]
const OWNER_REPO: &str = "alakazam777/uppwind-app";
pub const API_LATEST: &str = "https://api.github.com/repos/alakazam777/uppwind-app/releases/latest";
pub const DOWNLOAD_BASE: &str = "https://github.com/alakazam777/uppwind-app/releases/download/";
pub const RELEASES_PAGE: &str = "https://github.com/alakazam777/uppwind-app/releases/latest";
/// Le nom de l'exe Windows dans la release.
pub const EXE_NAME: &str = "uppwind-companion.exe";
pub const SUMS_NAME: &str = "SHA256SUMS";
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

const MAX_API: usize = 1024 * 1024;
const MAX_SUMS: usize = 16 * 1024;
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
}

impl Release {
    pub fn exe_url(&self) -> String {
        format!("{DOWNLOAD_BASE}{}/{EXE_NAME}", self.tag)
    }
    pub fn sums_url(&self) -> String {
        format!("{DOWNLOAD_BASE}{}/{SUMS_NAME}", self.tag)
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
    if find(SUMS_NAME).is_none() {
        return Err(format!("the latest release ({tag}) has no {SUMS_NAME}: it cannot be checked, so it is not installed"));
    }
    let exe_size = exe["size"].as_u64().filter(|s| *s > 0 && *s as usize <= MAX_EXE).ok_or("the release's exe has no plausible size")?;
    let exe_sha256 = match exe["digest"].as_str() {
        None => None,
        Some(d) => Some(d.strip_prefix("sha256:").filter(|h| is_hex64(h)).map(|h| h.to_ascii_lowercase()).ok_or("the release's exe has an unexpected digest")?),
    };
    Ok(Release { tag: tag.to_string(), version: tag[1..].to_string(), exe_size, exe_sha256 })
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
    crate::state_file::read(state_file).is_some_and(|v| v[STATE_KEY] == Value::Bool(true))
}

pub fn set_auto(state_file: &Path, on: bool) -> Result<bool, String> {
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

/// Telecharge l'exe de `rel` et son SHA256SUMS, les verifie, ecrit l'exe dans `dir` et le relit → son chemin et son
/// SHA-256. Rien n'est lance.
pub fn fetch_verified(rel: &Release, dir: &Path) -> Result<(PathBuf, String), String> {
    let sums = http_get(&rel.sums_url(), "application/octet-stream", MAX_SUMS)?;
    let bytes = http_get(&rel.exe_url(), "application/octet-stream", MAX_EXE)?;
    let sha = verify(&bytes, rel, &String::from_utf8_lossy(&sums))?;
    crate::log(&format!("update: {} downloaded, SHA-256 {sha} matches {SUMS_NAME}", rel.version));
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
            println!("downloaded and checked: {} (SHA-256 {sha}, the same as {SUMS_NAME}{})", file.display(), if c.release.exe_sha256.is_some() { " and GitHub's digest" } else { "" });
            println!("nothing was installed or started");
            0
        }
        Err(e) => {
            eprintln!("not verified: {e}");
            1
        }
    }
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

/// GET `url` (une constante de ce module) → le corps, au plus `max` octets, si la reponse est 200 et finit sur un
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

    fn api(tag: &str, digest: Option<&str>, with_sums: bool) -> String {
        let mut assets = vec![serde_json::json!({ "name": EXE_NAME, "size": 6, "digest": digest })];
        if with_sums {
            assets.push(serde_json::json!({ "name": SUMS_NAME, "size": 182 }));
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
        assert_eq!(r, Release { tag: "v0.4.0".into(), version: "0.4.0".into(), exe_size: 6, exe_sha256: Some("ab".repeat(32)) });
        // les adresses sont faites ici, jamais prises de la reponse
        assert_eq!(r.exe_url(), "https://github.com/alakazam777/uppwind-app/releases/download/v0.4.0/uppwind-companion.exe");
        assert_eq!(r.sums_url(), "https://github.com/alakazam777/uppwind-app/releases/download/v0.4.0/SHA256SUMS");
        assert_eq!(parse_release(&api("v0.4.0", None, true)).unwrap().exe_sha256, None);
        assert!(parse_release(&api("v0.4.0", None, false)).unwrap_err().contains("SHA256SUMS"), "no sums, no update");
        assert!(parse_release(&api("v0.4.0/../x", None, true)).is_err());
        assert!(parse_release(&api("0.4.0", None, true)).is_err(), "tags are v<x>.<y>.<z>");
        assert!(parse_release(&api("v0.4.0", Some("md5:abc"), true)).is_err());
        let mut v: Value = serde_json::from_str(&api("v0.4.0", None, true)).unwrap();
        v["prerelease"] = Value::Bool(true);
        assert!(parse_release(&v.to_string()).is_err());
        assert!(parse_release("<html>").is_err());
        assert!(parse_release(r#"{"tag_name":"v0.4.0","assets":[]}"#).is_err());
        // la vraie reponse de v0.3.0 (9 nov.), reduite
        let real = r#"{"tag_name":"v0.3.0","draft":false,"prerelease":false,"assets":[
            {"name":"SHA256SUMS","size":182,"digest":"sha256:360cc6ebbe8f14932953bbcb994413fdfaf3548adeef16296356ddf1f63e2cf1"},
            {"name":"uppwind-companion-macos.dmg","size":6820178},
            {"name":"uppwind-companion.exe","size":6332928,"digest":"sha256:5820c3e89a6ec1ef4f053ea3bcdfac6c0d64f43b567a0b56fa75ed16180d721a"}]}"#;
        let r = parse_release(real).unwrap();
        assert_eq!((r.version.as_str(), r.exe_size), ("0.3.0", 6332928));
        assert!(!is_newer(&r.version, "0.3.0"));
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
        let rel = Release { tag: "v9.0.0".into(), version: "9.0.0".into(), exe_size: 6, exe_sha256: Some(h.clone()) };
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
        for bad in ["http://github.com/x", "https://github.com.evil.example/x", "https://evil.example/github.com", "https://user@github.com/x", "https://github.com:444/x", "", "https://uppwind.lucasmoreau.fr/"] {
            assert!(!allowed_url(bad), "{bad}");
        }
        assert!(API_LATEST.contains(OWNER_REPO) && DOWNLOAD_BASE.contains(OWNER_REPO) && RELEASES_PAGE.contains(OWNER_REPO));
    }

    #[test]
    fn the_automatic_choice_is_kept_in_the_state_file_and_off_by_default() {
        let d = std::env::temp_dir().join(format!("uwc-upd-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("state.json");
        std::fs::write(&f, r#"{"token":"t"}"#).unwrap();
        assert!(!auto_on(&f), "off until ticked");
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
        let rel = Release { tag: "v0.5.0".into(), version: "0.5.0".into(), exe_size: 6, exe_sha256: None };
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

    #[test]
    fn nothing_downloads_without_a_newer_release() {
        let d = std::env::temp_dir();
        assert!(download_and_start(&d, false, &[], None).is_err());
        assert!(!view(None, None).auto, "no state file: off");
    }
}

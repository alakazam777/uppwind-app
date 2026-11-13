// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les messages des pages vers le compagnon (`window.ipc.postMessage`, dans le processus : aucun port, aucune adresse).
//! Du JSON, une liste fermee de commandes par page, des valeurs verifiees ; tout le reste est refuse. Les liens que la
//! page peut ouvrir sont nommes, jamais donnes : le compagnon a les adresses (pas d'adresse venue de la page).

use crate::wizard::steps::Choices;
use serde_json::Value;

/// Un message plus long est refuse (les choix d'une installation tiennent en quelques centaines d'octets).
pub const MAX_LEN: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Wizard,
    Status,
    Uninstall,
}

impl Page {
    pub fn name(self) -> &'static str {
        match self {
            Page::Wizard => "wizard",
            Page::Status => "status",
            Page::Uninstall => "uninstall",
        }
    }
}

/// Les liens que la page peut faire ouvrir dans le navigateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Studio,
    LoopMidi,
    /// la page de VB-CABLE : seulement dans l'aide de la fenetre d'etat, quand le retour audio a echoue
    VbCable,
    /// (11 nov.) la page de la derniere release (GitHub) : la ou la mise a jour depuis l'app ne se fait pas (macOS)
    Releases,
}

/// (11 nov.) Les gestes de la barre de titre de la page (la fenetre d'etat n'a plus celle de Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinAction {
    Minimize,
    /// agrandir, ou revenir a la taille d'avant
    Maximize,
    /// deplacer la fenetre (le bouton de la souris est enfonce sur la barre)
    Drag,
    /// tirer un bord ou un coin
    Resize(Edge),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Link {
    pub fn url(self) -> &'static str {
        match self {
            Link::Studio => crate::wizard::studio_url(),
            Link::LoopMidi => crate::wizard::LOOPMIDI_URL,
            Link::VbCable => crate::wizard::VBCABLE_URL,
            Link::Releases => crate::update::RELEASES_PAGE,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// la page s'est dessinee (l'essai --smoke-test l'attend)
    Ready,
    /// envoie-moi l'etat
    Init,
    Install,
    Customize,
    Next,
    Back,
    Choose(Choices),
    Recheck,
    Finish,
    Open(Link),
    /// copie le code d'appairage (le compagnon le connait : la page n'envoie rien)
    CopyCode,
    Close,
    Autostart(bool),
    /// la fenetre d'installation, depuis la fenetre d'etat
    SetupAgain,
    Confirm,
    Cancel,
    /// (9 nov., audit F1) armer (ou desarmer) : le studio peut filmer et piloter les fenetres de plugins, et prendre le
    /// son du DAW. Seulement depuis la fenetre d'etat (une page locale du compagnon, jamais le studio)
    Arm(bool),
    /// (9 nov., audit F2) un nouveau code d'appairage : l'ancien ne marche plus, le studio est deconnecte
    NewCode,
    /// (11 nov., discord.rs) permettre ou retirer la presence Discord ; seulement depuis la fenetre d'etat
    Discord(bool),
    /// (11 nov., update.rs) lire la derniere version publiee (GitHub) ; seulement depuis la fenetre d'etat
    CheckUpdate,
    /// installer la version trouvee (la page ne dit ni laquelle ni ou : le compagnon l'a lue lui-meme)
    Update,
    /// la case « Update automatically »
    AutoUpdate(bool),
    /// la barre de titre de la page
    Window(WinAction),
}

/// Un message d'une page → une demande permise sur cette page, ou pourquoi elle est refusee.
pub fn parse(page: Page, raw: &str) -> Result<Request, String> {
    if raw.len() > MAX_LEN {
        return Err("message too long".into());
    }
    let v: Value = serde_json::from_str(raw).map_err(|_| "not JSON".to_string())?;
    let cmd = v["cmd"].as_str().ok_or("no cmd")?;
    let req = match cmd {
        "ready" => Request::Ready,
        "init" => Request::Init,
        "close" => Request::Close,
        "install" => Request::Install,
        "customize" => Request::Customize,
        "next" => Request::Next,
        "back" => Request::Back,
        "recheck" => Request::Recheck,
        "finish" => Request::Finish,
        "confirm" => Request::Confirm,
        "cancel" => Request::Cancel,
        "setup" => Request::SetupAgain,
        "choose" => Request::Choose(serde_json::from_value(v["choices"].clone()).map_err(|e| format!("bad choices ({e})"))?),
        "copy" => match v["what"].as_str() {
            Some("code") => Request::CopyCode,
            _ => return Err("only the pairing code can be copied".into()),
        },
        "autostart" => Request::Autostart(v["on"].as_bool().ok_or("autostart needs on: true or false")?),
        "arm" => Request::Arm(v["on"].as_bool().ok_or("arm needs on: true or false")?),
        "newcode" => Request::NewCode,
        "discord" => Request::Discord(v["on"].as_bool().ok_or("discord needs on: true or false")?),
        "checkupdate" => Request::CheckUpdate,
        "update" => Request::Update,
        "autoupdate" => Request::AutoUpdate(v["on"].as_bool().ok_or("autoupdate needs on: true or false")?),
        "win" => Request::Window(match v["action"].as_str() {
            Some("minimize") => WinAction::Minimize,
            Some("maximize") => WinAction::Maximize,
            Some("drag") => WinAction::Drag,
            Some("resize") => WinAction::Resize(match v["edge"].as_str() {
                Some("n") => Edge::N,
                Some("s") => Edge::S,
                Some("e") => Edge::E,
                Some("w") => Edge::W,
                Some("ne") => Edge::NE,
                Some("nw") => Edge::NW,
                Some("se") => Edge::SE,
                Some("sw") => Edge::SW,
                _ => return Err("unknown edge".into()),
            }),
            _ => return Err("unknown window action".into()),
        }),
        "open" => Request::Open(match v["link"].as_str() {
            Some("studio") => Link::Studio,
            Some("loopmidi") => Link::LoopMidi,
            Some("vbcable") => Link::VbCable,
            Some("releases") => Link::Releases,
            _ => return Err("unknown link".into()),
        }),
        other => return Err(format!("unknown cmd {:?}", other.chars().take(24).collect::<String>())),
    };
    if !allowed(page, &req) {
        return Err(format!("{cmd} is not for the {} window", page.name()));
    }
    Ok(req)
}

fn allowed(page: Page, r: &Request) -> bool {
    use Request::*;
    match page {
        Page::Wizard => matches!(r, Ready | Init | Install | Customize | Next | Back | Choose(_) | Recheck | Finish | Open(Link::Studio | Link::LoopMidi) | CopyCode | Close),
        // (11 nov.) les reglages (Settings) et la mise a jour : cette fenetre locale seulement, jamais le studio (qui ne
        // parle qu'au serveur WebSocket, sans aucune de ces commandes)
        Page::Status => matches!(
            r,
            Ready | Init | Recheck | Open(_) | CopyCode | Close | Autostart(_) | SetupAgain | Arm(_) | NewCode | Discord(_) | CheckUpdate | Update | AutoUpdate(_) | Window(_)
        ),
        Page::Uninstall => matches!(r, Ready | Init | Confirm | Cancel | Close),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_window_has_its_own_commands() {
        assert_eq!(parse(Page::Wizard, r#"{"cmd":"install"}"#), Ok(Request::Install));
        assert_eq!(parse(Page::Wizard, r#"{"cmd":"open","link":"loopmidi"}"#), Ok(Request::Open(Link::LoopMidi)));
        assert!(parse(Page::Wizard, r#"{"cmd":"open","link":"vbcable"}"#).is_err(), "VB-CABLE is never offered at setup");
        assert!(parse(Page::Wizard, r#"{"cmd":"confirm"}"#).is_err(), "the uninstall's button");
        assert!(parse(Page::Wizard, r#"{"cmd":"autostart","on":true}"#).is_err());
        assert_eq!(parse(Page::Status, r#"{"cmd":"autostart","on":false}"#), Ok(Request::Autostart(false)));
        assert_eq!(parse(Page::Status, r#"{"cmd":"open","link":"vbcable"}"#), Ok(Request::Open(Link::VbCable)));
        assert!(parse(Page::Status, r#"{"cmd":"install"}"#).is_err());
        assert_eq!(parse(Page::Uninstall, r#"{"cmd":"confirm"}"#), Ok(Request::Confirm));
        assert!(parse(Page::Uninstall, r#"{"cmd":"copy","what":"code"}"#).is_err());
        // (9 nov., audit F1/F2) armer et changer de code : la fenetre d'etat seulement
        assert_eq!(parse(Page::Status, r#"{"cmd":"arm","on":true}"#), Ok(Request::Arm(true)));
        assert_eq!(parse(Page::Status, r#"{"cmd":"arm","on":false}"#), Ok(Request::Arm(false)));
        assert!(parse(Page::Status, r#"{"cmd":"arm","on":"yes"}"#).is_err());
        assert!(parse(Page::Status, r#"{"cmd":"arm"}"#).is_err());
        assert!(parse(Page::Wizard, r#"{"cmd":"arm","on":true}"#).is_err(), "only the status window arms");
        assert!(parse(Page::Uninstall, r#"{"cmd":"arm","on":true}"#).is_err());
        assert_eq!(parse(Page::Status, r#"{"cmd":"newcode"}"#), Ok(Request::NewCode));
        assert_eq!(parse(Page::Status, r#"{"cmd":"discord","on":true}"#), Ok(Request::Discord(true)));
        assert!(parse(Page::Status, r#"{"cmd":"discord"}"#).is_err());
        assert!(parse(Page::Wizard, r#"{"cmd":"discord","on":true}"#).is_err(), "only the status window turns it on");
        assert!(parse(Page::Wizard, r#"{"cmd":"newcode"}"#).is_err());
        // (11 nov.) la mise a jour et la barre de titre : la fenetre d'etat seulement
        assert_eq!(parse(Page::Status, r#"{"cmd":"checkupdate"}"#), Ok(Request::CheckUpdate));
        assert_eq!(parse(Page::Status, r#"{"cmd":"update"}"#), Ok(Request::Update));
        assert_eq!(parse(Page::Status, r#"{"cmd":"autoupdate","on":true}"#), Ok(Request::AutoUpdate(true)));
        assert!(parse(Page::Status, r#"{"cmd":"autoupdate","on":1}"#).is_err());
        assert_eq!(parse(Page::Status, r#"{"cmd":"open","link":"releases"}"#), Ok(Request::Open(Link::Releases)));
        assert_eq!(parse(Page::Status, r#"{"cmd":"win","action":"drag"}"#), Ok(Request::Window(WinAction::Drag)));
        assert_eq!(parse(Page::Status, r#"{"cmd":"win","action":"resize","edge":"se"}"#), Ok(Request::Window(WinAction::Resize(Edge::SE))));
        assert!(parse(Page::Status, r#"{"cmd":"win","action":"resize","edge":"x"}"#).is_err());
        assert!(parse(Page::Status, r#"{"cmd":"win","action":"close"}"#).is_err(), "closing is the close command");
        for page in [Page::Wizard, Page::Uninstall] {
            for cmd in [r#"{"cmd":"checkupdate"}"#, r#"{"cmd":"update"}"#, r#"{"cmd":"autoupdate","on":true}"#, r#"{"cmd":"win","action":"minimize"}"#, r#"{"cmd":"open","link":"releases"}"#] {
                assert!(parse(page, cmd).is_err(), "{cmd} on the {} window", page.name());
            }
        }
        // la page ne donne ni version ni adresse : elles sont ignorees
        assert_eq!(parse(Page::Status, r#"{"cmd":"update","url":"https://evil.example/x.exe","version":"9.9.9"}"#), Ok(Request::Update));
    }

    #[test]
    fn values_are_checked_and_links_are_names() {
        let c = parse(Page::Wizard, r#"{"cmd":"choose","choices":{"daws":["fl"],"fl_settings":false,"autostart":true,"open_studio":false}}"#).unwrap();
        assert_eq!(c, Request::Choose(Choices { daws: vec!["fl".into()], fl_settings: false, autostart: true, open_studio: false }));
        assert!(parse(Page::Wizard, r#"{"cmd":"choose","choices":{"daws":"fl"}}"#).is_err());
        assert!(parse(Page::Wizard, r#"{"cmd":"open","link":"https://evil.example"}"#).is_err(), "no address from the page");
        assert!(parse(Page::Wizard, r#"{"cmd":"open","url":"https://evil.example"}"#).is_err());
        assert!(parse(Page::Wizard, r#"{"cmd":"copy","what":"token"}"#).is_err());
        assert!(parse(Page::Status, r#"{"cmd":"autostart","on":"yes"}"#).is_err());
        assert!(parse(Page::Wizard, r#"{"cmd":"run","exe":"cmd.exe"}"#).is_err());
        assert!(parse(Page::Wizard, "not json").is_err());
        assert!(parse(Page::Wizard, r#"{"nocmd":1}"#).is_err());
        assert!(parse(Page::Wizard, &format!(r#"{{"cmd":"init","pad":"{}"}}"#, "x".repeat(MAX_LEN))).is_err(), "too long");
        assert_eq!(Link::Studio.url(), "https://beta.uppwind.com/atelier");
        assert!(!Link::Studio.url().contains('?'), "never a code in an address");
    }
}

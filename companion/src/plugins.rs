// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Uppwind
//! Les fenetres de plugins d'Ableton Live ; (10 nov.) sous macOS, aussi celles de Logic Pro.
//!
//! Mesure du 20 oct. (Live 12.4.6, Windows 10) : une fenetre de plugin est une fenetre de premier niveau du
//! processus de Live, POSSEDEE par sa fenetre principale, de style WS_EX_TOOLWINDOW, titree « <Plugin>/<Piste> »
//! (« Diva/2-Diva »). `plugin_windows` fait le tri sur une liste de fenetres brutes (testee avec une fausse liste) ;
//! `enumerate` (Windows seulement) remplit cette liste avec EnumWindows.
//!
//! Mesure du 9-10 nov. (Logic Pro 12.3.1, macOS 26.6) : la fenetre d'un plugin (AU) s'intitule, pour l'Accessibilite, du
//! nom de sa TRANCHE (« Deluxe Classic » 526×180, « Stereo Out » 526×200), sans le nom du plugin. Pour CGWindowList, son
//! titre est VIDE, meme avec « Enregistrement de l'ecran » (mesure du 11 nov. : le projet et « Logic Pro » y ont le
//! leur) : `enumerate` le demande a l'Accessibilite. Elle appartient a l'appli « Logic Pro »
//! (pas au service XPC d'Apple ou tournent nos AU), au niveau 0, et reste a l'ecran quand Logic n'est pas devant. Les
//! autres fenetres de Logic : le projet « <projet> - Tracks » (1728×1014), « Logic Pro » et « Choose a Project » (hors
//! de l'ecran ce soir-la). Le 11 nov., l'instrument de la meme piste (l'E-Piano de « Deluxe Classic », 516×462) s'est
//! ouvert au niveau 3 (flottant), Logic derriere : une fenetre de plugin de Logic est au niveau 0 ou 3.

use serde::Serialize;

pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;

/// Une fenetre de premier niveau telle que Windows la decrit.
#[derive(Debug, Clone, Default)]
pub struct RawWindow {
    pub hwnd: u64,
    pub pid: u32,
    /// Le nom du fichier du processus (« Ableton Live 12 Suite.exe »).
    pub exe: String,
    pub title: String,
    /// La classe de fenetre (AbletonVstPlugClass, Vst3PlugWindow…) : pas encore un critere, gardee pour la partie 2.
    #[allow(dead_code)]
    pub class: String,
    /// La fenetre proprietaire (0 : aucune).
    pub owner: u64,
    pub ex_style: u32,
    pub visible: bool,
    pub minimized: bool,
    /// La zone client, en pixels physiques (macOS : la fenetre entiere, en points).
    pub w: i32,
    pub h: i32,
    /// macOS : le niveau de la fenetre (kCGWindowLayer : 0 normal, 3 flottant…) ; 0 sous Windows
    pub layer: i32,
    /// (11 nov., relecture M1) macOS, Logic Pro : la fenetre porte le bouton de menu de l'affichage (AXMenuButton
    /// decrit « view » : Editor / Controls) que Logic met en haut de chacune de ses fenetres de plugins. Mesure : la
    /// fenetre de PSP VintageWarmer2 l'a ; celle du projet et « Control Surface Setup » (niveau 3, comme un plugin
    /// flottant) non. Lu par l'Accessibilite, seulement pour une fenetre que le reste n'exclut pas deja ; false sans
    /// elle, ou ailleurs que sous macOS.
    pub view_button: bool,
}

/// Une fenetre de plugin, telle que le studio la voit (`list`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PluginWindow {
    pub id: u64,
    pub title: String,
    pub plugin: String,
    pub track: String,
    pub w: i32,
    pub h: i32,
    pub minimized: bool,
}

/// Un processus d'Ableton Live ? (« Ableton Live 12 Suite.exe », « Ableton Live 12 Beta.exe », « Ableton Live 11 … »)
pub fn is_live_exe(exe: &str) -> bool {
    let e = exe.to_ascii_lowercase();
    let name = e.rsplit(['\\', '/']).next().unwrap_or(&e);
    name.starts_with("ableton live") && name.ends_with(".exe")
}

/// « Diva/2-Diva » → (« Diva », « 2-Diva »). Le nom du plugin ne contient pas de « / » d'habitude ; la piste, si :
/// on coupe au premier.
pub fn split_title(title: &str) -> Option<(String, String)> {
    let (p, t) = title.split_once('/')?;
    let p = p.trim();
    let t = t.trim();
    if p.is_empty() || t.is_empty() {
        return None;
    }
    Some((p.to_string(), t.to_string()))
}

/// macOS : l'appli de Live telle que CGWindowList la nomme (kCGWindowOwnerName) : « Live » (le nom de son
/// executable), ou « Ableton Live 12 Suite » selon les versions *(a tester)*.
pub fn is_live_app(owner: &str) -> bool {
    let o = owner.trim();
    o == "Live" || (o.starts_with("Ableton Live") && !o.to_ascii_lowercase().ends_with(".exe"))
}

/// Le tri : les fenetres d'un processus de Live, visibles, d'outil, possedees par une fenetre du meme processus,
/// titrees « plugin/piste ». Triees par titre (l'ordre de la liste ne bouge pas d'un appel a l'autre). (10 nov.) Sous
/// macOS, puis celles de Logic Pro (`logic_plugin_windows`).
pub fn plugin_windows(all: &[RawWindow]) -> Vec<PluginWindow> {
    if cfg!(target_os = "macos") {
        let mut v = plugin_windows_mac(all);
        v.extend(logic_plugin_windows(all));
        v
    } else {
        plugin_windows_win(all)
    }
}

/// macOS : l'appli Logic Pro telle que CGWindowList la nomme (kCGWindowOwnerName) : « Logic Pro » (11, 12), « Logic Pro
/// X » (10.x).
pub fn is_logic_app(owner: &str) -> bool {
    matches!(owner.trim(), "Logic Pro" | "Logic Pro X")
}

/// Les vues d'un projet de Logic, que sa fenetre porte apres son nom (« <projet> - Tracks ») : jamais un plugin.
const LOGIC_VIEWS: &[&str] = &[
    "Tracks", "Mixer", "Piano Roll", "Score", "Step Editor", "Step Sequencer", "Event List", "Audio Track Editor",
    "Audio File Editor", "Live Loops", "Smart Tempo Editor", "Project Audio", "Environment", "Transform", "Event Float",
];
/// Les fenetres de Logic lui-meme, qui ne sont pas des plugins (les titres mesures, et ses reglages).
const LOGIC_OWN: &[&str] = &["Logic Pro", "Logic Pro X", "Choose a Project", "Plug-in Manager", "Key Commands", "Settings", "Project Settings", "Control Surface Setup", "Tempo List"];

/// Les niveaux (kCGWindowLayer) d'une fenetre de plugin de Logic : 0, la fenetre normale (Notes et Return, 9-10 nov.),
/// et 3, la fenetre flottante (l'E-Piano, 11 nov.). Pas un dialogue modal (8), une bulle ni un menu (au-dela).
const LOGIC_PLUGIN_LAYERS: [i32; 2] = [0, 3];

/// (10 nov.) macOS : les fenetres de plugins de Logic Pro. Celles de l'appli « Logic Pro », a l'ecran, au niveau 0 ou 3
/// (LOGIC_PLUGIN_LAYERS), titrees : ni la fenetre d'un projet (« … - Tracks », « … - Mixer »), ni une fenetre de Logic
/// lui-meme (LOGIC_OWN), ni un dialogue (« Bounce “Stereo Out” », « Save »). Le titre est le nom de la tranche (le
/// plugin, Logic ne le dit pas) : `track` le porte, `plugin` reste vide (le studio montre alors le titre). Deux plugins
/// de la meme tranche (Notes et l'instrument) ont donc le meme titre.
/// (11 nov., relecture M1) La seconde barriere, seule : ce qui exclut deja une fenetre de Logic (hors de l'ecran, un
/// autre niveau que 0 ou 3, sans titre, Logic lui-meme, la vue d'un projet, un dialogue). Une fenetre qui la passe est
/// seulement candidate : `is_logic_plugin` lui demande encore le signe positif.
pub fn logic_candidate(title: &str, layer: i32, visible: bool) -> bool {
    let t = title.trim();
    visible
        && LOGIC_PLUGIN_LAYERS.contains(&layer)
        && !t.is_empty()
        && !LOGIC_OWN.contains(&t)
        && !LOGIC_VIEWS.iter().any(|v| t.strip_suffix(v).is_some_and(|head| head.ends_with(" - ")))
        && !["Bounce", "Export", "Save", "Open", "Import"].iter().any(|d| t == *d || t.starts_with(&format!("{d} ")))
}

/// (11 nov., relecture M1) Une fenetre de Logic est une fenetre de plugin si elle porte le signe positif (le bouton de
/// menu de l'affichage, `view_button`) ET passe la seconde barriere (`logic_candidate`). Avant, seule la liste
/// d'exclusion jouait : une fenetre de Logic au titre non liste (un panneau de reglages, une alerte titree) pouvait
/// etre montree au studio, puis pressee par ses gestes.
pub fn is_logic_plugin(title: &str, layer: i32, visible: bool, view_button: bool) -> bool {
    view_button && logic_candidate(title, layer, visible)
}

pub fn logic_plugin_windows(all: &[RawWindow]) -> Vec<PluginWindow> {
    let mut out: Vec<PluginWindow> = all
        .iter()
        .filter(|w| is_logic_app(&w.exe) && is_logic_plugin(&w.title, w.layer, w.visible, w.view_button))
        .map(|w| PluginWindow { id: w.hwnd, title: w.title.clone(), plugin: String::new(), track: w.title.trim().to_string(), w: w.w, h: w.h, minimized: w.minimized })
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then(a.id.cmp(&b.id)));
    out
}

/// Windows : le tri de `plugin_windows` (fenetres d'outil possedees).
pub fn plugin_windows_win(all: &[RawWindow]) -> Vec<PluginWindow> {
    let mut out: Vec<PluginWindow> = all
        .iter()
        .filter(|w| is_live_exe(&w.exe))
        .filter(|w| w.visible && w.owner != 0 && w.ex_style & WS_EX_TOOLWINDOW != 0)
        .filter(|w| all.iter().any(|o| o.hwnd == w.owner && o.pid == w.pid))
        .filter_map(|w| {
            let (plugin, track) = split_title(&w.title)?;
            Some(PluginWindow { id: w.hwnd, title: w.title.clone(), plugin, track, w: w.w, h: w.h, minimized: w.minimized })
        })
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then(a.id.cmp(&b.id)));
    out
}

/// macOS (26 oct.) : ni proprietaire ni style d'outil dans CGWindowList. Les fenetres de l'appli Live, a l'ecran, de
/// niveau normal ou flottant (0 a 8), titrees « plugin/piste » ; pas la fenetre principale (« Set … - Ableton Live 12
/// Suite », dont le nom du set peut contenir un « / »). *(a tester sur Live pour Mac : le titre « Diva/2-Diva »)*
pub fn plugin_windows_mac(all: &[RawWindow]) -> Vec<PluginWindow> {
    let mut out: Vec<PluginWindow> = all
        .iter()
        .filter(|w| is_live_app(&w.exe) && w.visible && (0..=8).contains(&w.layer))
        .filter(|w| !w.title.contains(" - Ableton Live"))
        .filter_map(|w| {
            let (plugin, track) = split_title(&w.title)?;
            Some(PluginWindow { id: w.hwnd, title: w.title.clone(), plugin, track, w: w.w, h: w.h, minimized: w.minimized })
        })
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then(a.id.cmp(&b.id)));
    out
}

/// (10 nov.) macOS : Logic Pro tourne-t-il (une fenetre de l'appli, quelle qu'elle soit ; son nom se lit sans
/// « Enregistrement de l'ecran ») ?
pub fn logic_running(all: &[RawWindow]) -> bool {
    cfg!(target_os = "macos") && all.iter().any(|w| is_logic_app(&w.exe))
}

/// Live tourne-t-il (une fenetre de son processus, quelle qu'elle soit) ?
pub fn live_running(all: &[RawWindow]) -> bool {
    all.iter().any(|w| is_live_exe(&w.exe) || (cfg!(target_os = "macos") && is_live_app(&w.exe)))
}

/// (10 nov.) Ce qui accompagne la liste (hello, windows) pour que le studio dise pourquoi elle est vide
/// (companion-link.js) : Live tourne-t-il ; sous macOS, Logic Pro aussi, et les titres des fenetres se lisent-ils
/// (« Enregistrement de l'ecran »). Ailleurs, None : le studio n'en dit rien.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    pub live: bool,
    pub logic: Option<bool>,
    pub titles: Option<bool>,
    /// (11 nov.) macOS : « Accessibilite » est-elle accordee ? Logic Pro ne nomme ses fenetres de plugins que pour elle
    /// (sans elle, sa liste reste vide), et ses gestes passent par elle.
    pub access: Option<bool>,
}

impl Running {
    /// `screen` : « Enregistrement de l'ecran » est accorde (screen_recording) ; `access` : « Accessibilite »
    /// (accessibility)
    pub fn of(all: &[RawWindow], screen: bool, access: bool) -> Running {
        let mac = cfg!(target_os = "macos");
        Running { live: live_running(all), logic: mac.then(|| logic_running(all)), titles: mac.then_some(screen), access: mac.then_some(access) }
    }

    /// Les champs d'un message : « live », et ceux que ce systeme connait (« logic », « titles », « access »).
    pub fn put(&self, v: &mut serde_json::Value) {
        v["live"] = self.live.into();
        if let Some(l) = self.logic {
            v["logic"] = l.into();
        }
        if let Some(t) = self.titles {
            v["titles"] = t.into();
        }
        if let Some(a) = self.access {
            v["access"] = a.into();
        }
    }
}

/// (11 nov.) macOS : « Accessibilite » est-elle accordee (lue sans rien demander : macOS ne la propose jamais de
/// lui-meme) ? Ailleurs : vrai.
pub fn accessibility() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::permissions().1
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// macOS : « Enregistrement de l'ecran » est-il accorde (lu sans rien demander) ? Sans lui, les titres des fenetres
/// des autres applis sont vides. Ailleurs : vrai.
pub fn screen_recording() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::permissions().0
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// macOS : les fenetres du systeme (CGWindowList) → la liste brute. Sans « Enregistrement de l'ecran », les titres
/// des autres applis sont vides : aucune fenetre de plugin ne sort (--list le dit). (11 nov.) Logic Pro, lui, ne donne
/// jamais a CGWindowList le titre de ses fenetres de plugins : pour une fenetre de Logic sans titre, a l'ecran, a un
/// niveau de plugin, le titre vient de l'Accessibilite (`macos::ax_windows`, une lecture par Logic et par appel), la
/// fenetre retrouvee par son cadre (`title_by_frame`). Sans l'autorisation « Accessibilite », elle reste sans titre.
#[cfg(target_os = "macos")]
pub fn enumerate() -> Vec<RawWindow> {
    let mut ax: std::collections::HashMap<i32, Vec<crate::macos::AxWin>> = std::collections::HashMap::new();
    crate::macos::all_windows()
        .into_iter()
        .map(|w| {
            let b = w.bounds;
            let mut title = w.title;
            let mut view_button = false;
            if w.onscreen && is_logic_app(&w.owner) && LOGIC_PLUGIN_LAYERS.contains(&w.layer) {
                let seen = ax.entry(w.pid).or_insert_with(|| crate::macos::ax_windows(w.pid));
                let frames: Vec<[f64; 4]> = seen.iter().map(|a| a.frame).collect();
                if let Some(a) = index_by_frame(&frames, [b.origin.x, b.origin.y, b.size.width, b.size.height]).map(|i| &seen[i]) {
                    if title.is_empty() {
                        title = a.title.clone();
                    }
                    // (11 nov., relecture M1) le signe positif, cherche seulement pour une candidate (pas la fenetre du
                    // projet, ni Logic lui-meme : la recherche coute plus que le titre)
                    view_button = logic_candidate(&title, w.layer, w.onscreen) && a.view_button();
                }
            }
            RawWindow {
                hwnd: w.id as u64,
                pid: w.pid.max(0) as u32,
                exe: w.owner,
                title,
                class: String::new(),
                owner: 0,
                ex_style: 0,
                visible: w.onscreen,
                minimized: false,
                w: b.size.width.round() as i32,
                h: b.size.height.round() as i32,
                layer: w.layer,
                view_button,
            }
        })
        .collect()
}

/// (12 nov., option a) Les points ou l'on regarde si une autre fenetre de Logic couvre celle-ci : seize, quatre colonnes
/// sur quatre rangees (12,5, 37,5, 62,5 et 87,5 % de la largeur et de la hauteur), sur TOUTE la fenetre, barre et pied
/// compris : les gestes du studio y vont aussi. Mesure le 12 nov. : Return (526 × 200) sous Notes, l'E-Piano dont le
/// haut est sous Return ; les douze points du test « opaque » ne voient pas ce haut, ceux-ci si.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn cover_probe_points(frame: [f64; 4]) -> Vec<(f64, f64)> {
    let [x, y, w, h] = frame;
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    const AT: [f64; 4] = [0.125, 0.375, 0.625, 0.875];
    AT.iter().flat_map(|fy| AT.iter().map(move |fx| (x + w * fx, y + h * fy))).collect()
}

/// (11 nov., 23 h) Les points ou le test « opaque » cherche un bouton : douze, quatre colonnes (20, 40, 60, 80 % de la
/// largeur) sur trois rangees (20, 50, 80 % de la hauteur sous la barre de Logic, `header` points). Aucun sans place
/// sous la barre.
/// (12 nov.) Ni dans le pied de la fenetre (`footer` points), ou Logic ecrit le nom du plugin : il n'y a jamais de
/// bouton la, et une fenetre couverte par une autre pouvait n'y montrer que ce nom, donc passer pour opaque.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn opaque_probe_points(frame: [f64; 4], header: f64, footer: f64) -> Vec<(f64, f64)> {
    let [x, y, w, h] = frame;
    let body = h - header - footer;
    if w <= 0.0 || body <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(12);
    for fy in [0.2, 0.5, 0.8] {
        for fx in [0.2, 0.4, 0.6, 0.8] {
            out.push((x + w * fx, y + header + body * fy));
        }
    }
    out
}

/// (12 nov.) Ce que le test « opaque » a conclu sous son delai d'ensemble (`opaque_within`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpaqueTest {
    /// le verdict : vrai seulement si le test est alle au bout a temps, sans trouver un seul bouton, avec au moins la
    /// moitie de ses points dans la fenetre
    pub opaque: bool,
    /// les points sondes : le test s'arrete au premier bouton trouve, ou a son delai
    pub probed: usize,
    /// (12 nov.) ceux qui sont tombes dans la fenetre (les autres : sur une fenetre posee devant)
    pub in_window: usize,
    /// arrete par son delai (alors jamais opaque)
    pub late: bool,
}

/// (11 nov., 23 h) Le dessin d'un plugin est-il opaque, d'apres ses points sondes ? Chacun donne `Some(true)` : un
/// bouton a appuyer ou a tourner y est ; `Some(false)` : le point tombe dans la fenetre, mais rien n'y est ; `None` : le
/// point n'est pas dans la fenetre (une autre la couvre). Opaque : aucun bouton, et (12 nov.) au moins la moitie des
/// points dans la fenetre. Avant, un seul suffisait : Return, couvert par Notes, passait pour opaque d'apres quatre
/// points sur douze, tous sur le nom ecrit au pied de sa fenetre (mesure). Une fenetre surtout couverte n'en montre pas
/// assez pour conclure.
/// (12 nov.) Sous un delai d'ensemble. `probes` est paresseux : chaque point interroge Logic au moment ou il est lu.
/// `late` dit si le delai est passe ; il est relu apres chaque point. Le test s'arrete au premier bouton trouve (pas
/// opaque). Passe le delai, il conclut « pas opaque », sans compter le dernier point, dont les appels ont pu etre
/// coupes. Ce signe ne sert qu'au texte du studio et au bouton Controls mis en avant : le donner a tort ferait croire
/// que des clics qui marchent ne passent pas.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn opaque_within(probes: impl IntoIterator<Item = Option<bool>>, mut late: impl FnMut() -> bool) -> OpaqueTest {
    let mut test = OpaqueTest::default();
    for found in probes {
        test.probed += 1;
        if late() {
            test.late = true;
            return test;
        }
        match found {
            Some(true) => {
                test.in_window += 1;
                return test;
            }
            Some(false) => test.in_window += 1,
            None => {}
        }
    }
    test.opaque = test.in_window > 0 && test.in_window * 2 >= test.probed;
    test
}

/// (12 nov.) Ce qu'une recherche sous un delai d'ensemble a trouve (`first_within`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub enum Search<T> {
    Found(T),
    /// rien : tout est parcouru, ou la limite d'elements est atteinte
    Absent,
    /// arretee par son delai : on n'en sait rien
    Late,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl<T> Search<T> {
    /// L'element trouve. Rien sinon, delai passe compris : une barriere qui cherche un signe echoue fermee.
    pub fn found(self) -> Option<T> {
        match self {
            Search::Found(t) => Some(t),
            Search::Absent | Search::Late => None,
        }
    }
}

/// (12 nov.) La recherche du bouton de menu de l'affichage (`macos::ax_view_button`), ecrite sans l'Accessibilite pour
/// etre essayee. En largeur depuis `root`, sur `depth` niveaux d'enfants, `max` elements examines au plus : le premier
/// qui `fits`. `late` (le delai d'ensemble) est relu avant chaque appel a `children` et a `fits`, qui interrogent Logic
/// une ou deux fois chacun. Passe le delai : `Late`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn first_within<N>(
    root: N,
    depth: usize,
    max: usize,
    mut children: impl FnMut(&N) -> Vec<N>,
    mut fits: impl FnMut(&N) -> bool,
    mut late: impl FnMut() -> bool,
) -> Search<N> {
    let mut level = vec![root];
    let mut seen = 0;
    for _ in 0..depth {
        let mut next = Vec::new();
        for n in &level {
            if late() {
                return Search::Late;
            }
            for k in children(n) {
                seen += 1;
                if seen > max {
                    return Search::Absent;
                }
                if late() {
                    return Search::Late;
                }
                if fits(&k) {
                    return Search::Found(k);
                }
                next.push(k);
            }
        }
        level = next;
    }
    Search::Absent
}

/// (11 nov.) Le menu d'affichage d'une fenetre de plugin de Logic (mesure avec PSP VintageWarmer2 : « Controls », le
/// nom du plugin, puis une ligne vide) : l'element a choisir pour passer a « Controls », ou au dessin du plugin (le
/// premier element titre qui n'est pas « Controls »).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn logic_view_item(titles: &[String], controls: bool) -> Option<usize> {
    titles.iter().position(|t| if controls { t == "Controls" } else { !t.trim().is_empty() && t != "Controls" })
}

/// (11 nov.) Le titre d'une fenetre retrouvee par son cadre (x, y, largeur, hauteur, en points), a un point pres,
/// parmi celles que l'Accessibilite decrit. Deux fenetres exactement l'une sur l'autre : la premiere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn title_by_frame(seen: &[(String, [f64; 4])], frame: [f64; 4]) -> Option<&str> {
    let frames: Vec<[f64; 4]> = seen.iter().map(|(_, r)| *r).collect();
    index_by_frame(&frames, frame).map(|i| seen[i].0.as_str())
}

/// (11 nov.) L'index de la fenetre dont le cadre est `frame`, a un point pres (la premiere s'il y en a deux).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn index_by_frame(frames: &[[f64; 4]], frame: [f64; 4]) -> Option<usize> {
    frames.iter().position(|r| r.iter().zip(frame).all(|(a, b)| (a - b).abs() <= 1.0))
}

/// Le titre que Live donne a la fenetre d'un device : « <device>/<piste> ».
pub fn expected_title(device: &str, track: &str) -> String {
    format!("{device}/{track}")
}

#[cfg(windows)]
pub use win::enumerate;

#[cfg(not(any(windows, target_os = "macos")))]
pub fn enumerate() -> Vec<RawWindow> {
    Vec::new()
}

#[cfg(windows)]
mod win {
    use super::RawWindow;
    use std::collections::HashMap;
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetClientRect, GetWindow, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        GWL_EXSTYLE, GW_OWNER,
    };

    unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> BOOL {
        let v = &mut *(lp.0 as *mut Vec<isize>);
        v.push(hwnd.0 as isize);
        BOOL(1)
    }

    pub fn exe_of(pid: u32) -> String {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
            let mut buf = [0u16; 1024];
            let mut n = buf.len() as u32;
            let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
            let _ = CloseHandle(h);
            if r.is_err() {
                return String::new();
            }
            let full = String::from_utf16_lossy(&buf[..n as usize]);
            full.rsplit('\\').next().unwrap_or("").to_string()
        }
    }

    pub fn describe(h: HWND, exes: &mut HashMap<u32, String>) -> RawWindow {
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let exe = exes.entry(pid).or_insert_with(|| exe_of(pid)).clone();
            let mut t = [0u16; 512];
            let n = GetWindowTextW(h, &mut t).max(0) as usize;
            let mut c = [0u16; 256];
            let cn = GetClassNameW(h, &mut c).max(0) as usize;
            let owner = GetWindow(h, GW_OWNER).map(|o| o.0 as u64).unwrap_or(0);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            let mut r = RECT::default();
            let _ = GetClientRect(h, &mut r);
            RawWindow {
                hwnd: h.0 as u64,
                pid,
                exe,
                title: String::from_utf16_lossy(&t[..n]),
                class: String::from_utf16_lossy(&c[..cn]),
                owner,
                ex_style: ex,
                visible: IsWindowVisible(h).as_bool(),
                minimized: IsIconic(h).as_bool(),
                w: r.right - r.left,
                h: r.bottom - r.top,
                layer: 0,
                view_button: false,
            }
        }
    }

    /// Toutes les fenetres de premier niveau. Le nom du processus n'est lu que pour les fenetres visibles et celles
    /// qui possedent une fenetre (assez pour reconnaitre Live), une fois par processus.
    pub fn enumerate() -> Vec<RawWindow> {
        let mut handles: Vec<isize> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<isize> as isize));
        }
        let mut exes: HashMap<u32, String> = HashMap::new();
        handles.into_iter().map(|h| describe(HWND(h as *mut core::ffi::c_void), &mut exes)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = "Ableton Live 12 Suite.exe";

    fn win(hwnd: u64, pid: u32, exe: &str, title: &str, owner: u64, tool: bool) -> RawWindow {
        RawWindow {
            hwnd,
            pid,
            exe: exe.into(),
            title: title.into(),
            class: "AbletonVstPlugClass".into(),
            owner,
            ex_style: if tool { WS_EX_TOOLWINDOW } else { 0 },
            visible: true,
            minimized: false,
            w: 800,
            h: 500,
            layer: 0,
            view_button: false,
        }
    }

    /// (26 oct.) macOS : les fenetres de l'appli « Live », titrees « plugin/piste », pas la fenetre principale.
    #[test]
    fn finds_live_plugin_windows_on_macos() {
        let mac = |id: u64, owner: &str, title: &str, layer: i32, on: bool| RawWindow { hwnd: id, pid: 100, exe: owner.into(), title: title.into(), visible: on, w: 600, h: 400, layer, ..Default::default() };
        let d = vec![
            mac(1, "Live", "My/Set [My/Set] - Ableton Live 12 Suite", 0, true),
            mac(2, "Live", "Diva/2-Diva", 0, true),
            mac(3, "Live", "Serum 2/3-Lead / Pad", 3, true),
            mac(4, "Live", "Pro-Q 4/5-Master", 0, false),
            mac(5, "Google Chrome", "Diva/2-Diva", 0, true),
            mac(6, "Live", "Diva/2-Diva", 25, true), // un menu, une bulle : pas une fenetre de plugin
            mac(7, "Live", "", 0, true),            // sans « Enregistrement de l'ecran » : pas de titre
        ];
        let ids: Vec<u64> = plugin_windows_mac(&d).iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![2, 3]);
        assert!(is_live_app("Live") && is_live_app("Ableton Live 12 Suite") && !is_live_app("Ableton Live 12 Suite.exe") && !is_live_app("Livestream"));
    }

    /// (11 nov., relecture M1) La decision : le signe positif d'abord, puis la seconde barriere.
    #[test]
    fn a_logic_window_is_a_plugin_only_with_the_view_button() {
        // le signe et la seconde barriere : une fenetre de plugin (au niveau 0, ou flottante au niveau 3)
        assert!(is_logic_plugin("Deluxe Classic", 0, true, true));
        assert!(is_logic_plugin("Stereo Out", 3, true, true));
        // sans le signe : jamais, quel que soit le titre (un panneau, une alerte titree, une fenetre de reglages)
        for t in ["Deluxe Classic", "Audio Settings", "Are you sure?", "Untitled", "Control Surface Setup"] {
            assert!(!is_logic_plugin(t, 0, true, false), "{t}");
            assert!(!is_logic_plugin(t, 3, true, false), "{t}");
        }
        // avec le signe, la seconde barriere tient encore
        assert!(!is_logic_plugin("Deluxe Classic", 8, true, true), "a modal level");
        assert!(!is_logic_plugin("Deluxe Classic", 0, false, true), "off screen");
        assert!(!is_logic_plugin("  ", 0, true, true), "no title");
        assert!(!is_logic_plugin("Settings", 0, true, true), "Logic itself");
        assert!(!is_logic_plugin("My Song - Tracks", 0, true, true), "a project view");
        assert!(!is_logic_plugin("Bounce “Stereo Out”", 0, true, true), "a dialog");
        // la candidate seule (ce qui decide si le signe est cherche)
        assert!(logic_candidate("Audio Settings", 0, true) && !logic_candidate("Control Surface Setup", 3, true));
    }

    /// (11 nov.) Le cadre d'une fenetre retrouve son index parmi celles de l'Accessibilite (a un point pres).
    #[test]
    fn finds_a_window_by_its_frame() {
        let f = [[193.0, 146.0, 1524.0, 920.0], [538.0, 251.0, 516.0, 462.0]];
        assert_eq!(index_by_frame(&f, [538.6, 251.0, 516.0, 462.0]), Some(1));
        assert_eq!(index_by_frame(&f, [536.0, 251.0, 516.0, 462.0]), None);
        assert_eq!(index_by_frame(&[], [0.0; 4]), None);
    }

    /// (12 nov., option a) Ou l'on regarde si une autre fenetre de Logic couvre celle-ci : seize points sur toute la
    /// fenetre, barre et pied compris, tous dedans.
    #[test]
    fn the_cover_test_looks_at_the_whole_window() {
        let pts = cover_probe_points([0.0, 0.0, 800.0, 400.0]);
        assert_eq!(pts.len(), 16);
        assert_eq!((pts[0], pts[15]), ((100.0, 50.0), (700.0, 350.0)));
        // l'E-Piano (mesure : 516 × 462 a 138,340, son haut sous Return et Notes jusqu'a 503) : la premiere rangee y
        // tombe ; les douze points du test « opaque », entre la barre et le pied, commencent plus bas
        let ep = cover_probe_points([138.0, 340.0, 516.0, 462.0]);
        assert!(ep.iter().any(|&(_, y)| y < 503.0), "{ep:?}");
        assert!(opaque_probe_points([138.0, 340.0, 516.0, 462.0], 100.0, 34.0).iter().all(|&(_, y)| y > 503.0));
        assert!(ep.iter().all(|&(x, y)| x > 138.0 && x < 654.0 && y > 340.0 && y < 802.0), "inside");
        assert!(cover_probe_points([0.0, 0.0, 0.0, 100.0]).is_empty());
    }

    /// (11 nov., 23 h) Le test « opaque » : douze points sous la barre, et un seul bouton trouve suffit.
    #[test]
    fn a_view_is_opaque_only_if_no_probe_finds_a_control() {
        let pts = opaque_probe_points([100.0, 50.0, 500.0, 500.0], 100.0, 0.0);
        assert_eq!(pts.len(), 12);
        assert_eq!((pts[0], pts[11]), ((200.0, 230.0), (500.0, 470.0)));
        assert!(pts.iter().all(|&(x, y)| x > 100.0 && x < 600.0 && y > 150.0 && y < 550.0), "under the header, inside");
        assert!(opaque_probe_points([0.0, 0.0, 500.0, 90.0], 100.0, 0.0).is_empty(), "no room under the header");
        // (12 nov.) ni dans le pied : Return (526 × 200, mesure), dont le nom occupe les 34 derniers points
        let ret = opaque_probe_points([45.0, 303.0, 526.0, 200.0], 100.0, 34.0);
        assert_eq!(ret.len(), 12);
        assert!(ret.iter().all(|&(_, y)| y > 403.0 && y < 469.0), "between the header and the footer: {ret:?}");
        assert!(opaque_probe_points([0.0, 0.0, 500.0, 134.0], 100.0, 34.0).is_empty(), "no room between them");
        // (12 nov.) le verdict seul, sans delai
        let opaque_of = |found: &[Option<bool>]| opaque_within(found.iter().copied(), || false).opaque;
        // PSP VintageWarmer2 (mesure : la fenetre seule, partout) : opaque
        assert!(opaque_of(&[Some(false); 12]));
        // l'E-Piano a 50 % : le centre sur un decor, des boutons ailleurs : pas opaque
        let mut epiano = [Some(false); 12];
        epiano[3] = Some(true);
        assert!(!opaque_of(&epiano));
        // couvert par une autre fenetre : on ne conclut pas
        assert!(!opaque_of(&[None; 12]));
        assert!(!opaque_of(&[]));
        // (12 nov.) en partie couvert : au moins la moitie des points dans la fenetre pour conclure. Return sous Notes
        // (mesure, avant que le pied soit laisse de cote) : huit points sur Notes, quatre sur son nom, aucun bouton
        let mut ret = [None; 12];
        ret[8..].fill(Some(false));
        assert!(!opaque_of(&ret), "4 of 12 in the window: not enough");
        ret[6..].fill(Some(false));
        assert!(opaque_of(&ret), "6 of 12: enough");
        assert!(!opaque_of(&[None, Some(false), None]) && opaque_of(&[None, Some(false), Some(false)]));
    }

    /// (12 nov.) Le test « opaque » sur des points simules, avec une horloge simulee : chaque point lu l'avance de
    /// `cost` ms, et le delai est de `budget` ms. → le verdict, et le nombre de points vraiment demandes a « Logic ».
    fn opaque_sim(found: &[Option<bool>], cost: u64, budget: u64) -> (OpaqueTest, usize) {
        let (asked, clock) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
        let probes = found.iter().map(|f| {
            asked.set(asked.get() + 1);
            clock.set(clock.get() + cost);
            *f
        });
        let test = opaque_within(probes, || clock.get() >= budget);
        (test, asked.get())
    }

    /// (12 nov.) Le test « opaque » s'arrete au premier bouton : les points suivants ne sont jamais demandes.
    #[test]
    fn the_opaque_test_stops_at_the_first_control() {
        let mut found = [Some(false); 12];
        found[2] = Some(true);
        found[7] = Some(true);
        let (test, asked) = opaque_sim(&found, 0, 300);
        assert_eq!(test, OpaqueTest { opaque: false, probed: 3, in_window: 3, late: false });
        assert_eq!(asked, 3, "nothing is asked after the first control");
        // aucun bouton : tous les points sont demandes, et c'est opaque (PSP VintageWarmer2)
        let psp = OpaqueTest { opaque: true, probed: 12, in_window: 12, late: false };
        assert_eq!(opaque_sim(&[Some(false); 12], 0, 300), (psp, 12));
        // l'E-Piano a 50 % : le premier point sur un decor, un bouton au deuxieme
        let mut epiano = [Some(false); 12];
        epiano[1] = Some(true);
        assert_eq!(opaque_sim(&epiano, 0, 300).0, OpaqueTest { opaque: false, probed: 2, in_window: 2, late: false });
    }

    /// (12 nov.) Passe son delai, le test « opaque » s'arrete et dit « pas opaque » : un Logic lent ne retient plus le
    /// compagnon pendant douze points.
    #[test]
    fn the_opaque_test_is_not_opaque_past_its_deadline() {
        // 100 ms par point, 300 ms de delai : trois points, puis l'arret, meme si rien n'a ete trouve
        let (test, asked) = opaque_sim(&[Some(false); 12], 100, 300);
        assert_eq!(test, OpaqueTest { opaque: false, probed: 3, in_window: 2, late: true });
        assert_eq!(asked, 3, "the fourth point is never asked");
        // le point fini apres le delai ne compte pas : compte, il aurait fait conclure « opaque » (deux points sur trois)
        let cut = OpaqueTest { opaque: false, probed: 3, in_window: 1, late: true };
        assert_eq!(opaque_sim(&[None, Some(false), Some(false)], 100, 300).0, cut);
        assert!(opaque_sim(&[None, Some(false), Some(false)], 100, 301).0.opaque, "just in time");
        // a temps : 20 ms par point, 240 ms en tout
        let psp = OpaqueTest { opaque: true, probed: 12, in_window: 12, late: false };
        assert_eq!(opaque_sim(&[Some(false); 12], 20, 300).0, psp);
        // un bouton trouve avant le delai : pas opaque, et pas en retard
        let mut found = [Some(false); 12];
        found[1] = Some(true);
        assert_eq!(opaque_sim(&found, 100, 300).0, OpaqueTest { opaque: false, probed: 2, in_window: 2, late: false });
        // aucun point : rien a conclure
        assert_eq!(opaque_sim(&[], 100, 300), (OpaqueTest::default(), 0));
    }

    /// (12 nov.) Un arbre d'elements simule pour la recherche du bouton de menu de l'affichage : le noeud n a pour
    /// enfants 10n+1, 10n+2 et 10n+3 (le premier niveau : 1, 2, 3 ; le deuxieme : 11 a 33 ; le troisieme : 111 a 333),
    /// ou `wide` enfants pour la racine (0) si `wide` n'est pas nul. Chaque appel a `children` ou a `fits` avance
    /// l'horloge simulee de `cost` ms ; le delai est de `budget` ms. → ce que la recherche trouve, et ses appels.
    fn search_sim(button: &[u32], wide: u32, cost: u64, budget: u64) -> (Search<u32>, u64) {
        let (calls, clock) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
        let tick = || {
            calls.set(calls.get() + 1);
            clock.set(clock.get() + cost);
        };
        let children = |n: &u32| {
            tick();
            match (*n, wide) {
                (0, w) if w > 0 => (1..=w).map(|k| 10_000 + k).collect(),
                (n, _) if n < 1000 => (1..=3).map(|k| n * 10 + k).collect(),
                _ => Vec::new(),
            }
        };
        let fits = |n: &u32| {
            tick();
            button.contains(n)
        };
        let found = first_within(0, 3, 400, children, fits, || clock.get() >= budget);
        (found, calls.get())
    }

    /// (12 nov.) La recherche du bouton de menu de l'affichage : en largeur, sur trois niveaux, 400 elements au plus, le
    /// premier trouve.
    #[test]
    fn the_view_menu_search_goes_by_level_and_is_bounded() {
        assert_eq!(search_sim(&[123], 0, 0, 300).0, Search::Found(123), "on the third level");
        assert_eq!(search_sim(&[111, 23], 0, 0, 300).0, Search::Found(23), "the second level before the third");
        assert_eq!(search_sim(&[2, 1], 0, 0, 300), (Search::Found(1), 2), "the first one, at the first call");
        assert_eq!(search_sim(&[1111], 0, 0, 300).0, Search::Absent, "not on a fourth level");
        // 500 enfants sans bouton : 400 examines, puis l'arret (1 appel a `children`, 400 a `fits`)
        assert_eq!(search_sim(&[], 500, 0, 300), (Search::Absent, 401));
        assert_eq!(search_sim(&[10_400], 500, 0, 300).0, Search::Found(10_400), "the 400th is still examined");
        assert_eq!(search_sim(&[10_401], 500, 0, 300).0, Search::Absent, "not the 401st");
    }

    /// (12 nov.) Passe son delai, la recherche s'arrete et ne trouve rien : la fenetre n'est pas prise pour une fenetre
    /// de plugin (ni listee, ni pressee, ni changee d'affichage). La barriere echoue fermee.
    #[test]
    fn the_view_menu_search_fails_closed_past_its_deadline() {
        // 10 ms par appel, 50 ms de delai : cinq appels, puis l'arret, avant le bouton du troisieme niveau
        let (found, calls) = search_sim(&[123], 0, 10, 50);
        assert_eq!((found, calls), (Search::Late, 5));
        assert_eq!(search_sim(&[123], 0, 10, 50).0.found(), None);
        // la meme fenetre, avec assez de temps : le bouton y est, au 24e appel (240 ms)
        assert_eq!(search_sim(&[123], 0, 10, 1000), (Search::Found(123), 24));
        // le delai deja passe : aucun appel a Logic
        assert_eq!(search_sim(&[1], 0, 10, 0), (Search::Late, 0));
        // la decision : sans le signe, jamais une fenetre de plugin
        let sign = search_sim(&[123], 0, 10, 50).0.found().is_some();
        assert!(!is_logic_plugin("Deluxe Classic", 3, true, sign));
        assert!(is_logic_plugin("Deluxe Classic", 3, true, search_sim(&[123], 0, 10, 1000).0.found().is_some()));
    }

    /// (11 nov.) Le menu d'affichage d'une fenetre de Logic, tel que PSP VintageWarmer2 l'a montre.
    #[test]
    fn picks_the_logic_view_menu_item() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let menu = t(&["Controls", "PSP VintageWarmer2", ""]);
        assert_eq!(logic_view_item(&menu, true), Some(0));
        assert_eq!(logic_view_item(&menu, false), Some(1));
        assert_eq!(logic_view_item(&t(&["", "Controls"]), false), None, "no plugin view");
        assert_eq!(logic_view_item(&t(&["Vintage Electric Piano"]), true), None);
    }

    /// (11 nov.) Logic ne nomme ses fenetres de plugins que pour l'Accessibilite : la fenetre est retrouvee par son cadre.
    #[test]
    fn names_a_logic_window_by_its_frame() {
        let seen = vec![
            ("Uppwind Link - essai Logic - Tracks".to_string(), [193.0, 146.0, 1524.0, 920.0]),
            ("Deluxe Classic".to_string(), [538.0, 251.0, 516.0, 462.0]),
        ];
        assert_eq!(title_by_frame(&seen, [538.0, 251.0, 516.0, 462.0]), Some("Deluxe Classic"));
        assert_eq!(title_by_frame(&seen, [538.4, 250.6, 516.0, 461.5]), Some("Deluxe Classic")); // a un point pres
        assert_eq!(title_by_frame(&seen, [540.0, 251.0, 516.0, 462.0]), None);
        assert_eq!(title_by_frame(&[], [538.0, 251.0, 516.0, 462.0]), None);
    }

    /// (10 nov.) Logic Pro 12.3.1 : les fenetres mesurees (titres, tailles, niveaux), et ce qui n'est pas un plugin.
    /// (11 nov.) L'instrument de la piste, ouvert au niveau 3 (flottant), en est une aussi. (11 nov., relecture M1)
    /// Seules les fenetres qui portent le bouton de menu de l'affichage (`view_button`) en sont : une fenetre de Logic au
    /// titre non liste, sans ce bouton, n'est plus montree.
    #[test]
    fn finds_logic_plugin_windows_on_macos() {
        let mac = |id: u64, owner: &str, title: &str, w: i32, h: i32, layer: i32, on: bool| RawWindow { hwnd: id, pid: 300, exe: owner.into(), title: title.into(), visible: on, w, h, layer, ..Default::default() };
        let plugin = |id: u64, owner: &str, title: &str, w: i32, h: i32, layer: i32| RawWindow { view_button: true, ..mac(id, owner, title, w, h, layer, true) };
        let d = vec![
            mac(9781, "Logic Pro", "Uppwind Link - essai Logic - Tracks", 1728, 1014, 0, true), // le projet
            plugin(9779, "Logic Pro", "Deluxe Classic", 526, 180, 0),                        // Notes (MIDI FX), sur sa piste
            plugin(9999, "Logic Pro", "Deluxe Classic", 516, 462, 3),                        // l'E-Piano de la piste, flottant
            mac(9797, "Logic Pro", "Pad", 900, 600, 8, true),                                // un dialogue modal
            plugin(9780, "Logic Pro", "Stereo Out", 526, 200, 0),                            // Return, sur la sortie
            mac(9778, "Logic Pro", "Logic Pro", 415, 105, 0, true),                          // une fenetre de Logic lui-meme
            mac(9775, "Logic Pro", "Choose a Project", 1042, 626, 0, true),
            mac(9790, "Logic Pro", "Uppwind Link - essai Logic - Mixer", 1200, 700, 0, true),
            mac(9791, "Logic Pro", "Bounce “Stereo Out”", 700, 500, 0, true),
            mac(9792, "Logic Pro", "Alchemy", 900, 600, 0, false), // hors de l'ecran
            mac(9793, "Logic Pro", "Pad", 900, 600, 25, true),     // un menu, une bulle
            mac(9794, "Logic Pro", "", 526, 180, 0, true),         // sans « Enregistrement de l'ecran » : pas de titre
            mac(9795, "Google Chrome", "Deluxe Classic", 526, 180, 0, true),
            plugin(9796, "Logic Pro X", "Bass - Synth", 600, 400, 0), // Logic 10.x ; un « - » qui n'est pas une vue
            // (relecture M1) des fenetres de Logic au titre que la liste ne connait pas, sans le bouton : pas des plugins
            mac(9801, "Logic Pro", "Audio Settings", 600, 400, 0, true),
            mac(9802, "Logic Pro", "Are you sure?", 400, 160, 3, true),
            mac(9803, "Logic Pro", "Control Surface Setup", 862, 814, 3, true), // mesure : niveau 3, sans le bouton
            // le bouton, mais une fenetre que la seconde barriere exclut deja : pas un plugin non plus
            plugin(9804, "Logic Pro", "Settings", 600, 400, 0),
            plugin(9805, "Logic Pro", "Deluxe Classic", 526, 180, 8),
        ];
        let got = logic_plugin_windows(&d);
        let ids: Vec<u64> = got.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![9796, 9779, 9999, 9780]);
        assert_eq!((got[1].title.as_str(), got[1].plugin.as_str(), got[1].track.as_str(), got[1].w, got[1].h), ("Deluxe Classic", "", "Deluxe Classic", 526, 180));
        assert_eq!((got[2].title.as_str(), got[2].w, got[2].h), ("Deluxe Classic", 516, 462));
        assert!(is_logic_app("Logic Pro") && is_logic_app("Logic Pro X") && !is_logic_app("Logic") && !is_logic_app("Logic Pro.app"));
        // ni Live ni Logic ne prennent les fenetres de l'autre
        assert!(plugin_windows_mac(&d).is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(plugin_windows(&d).len(), 4);
        }
    }

    #[test]
    fn says_what_runs_for_an_empty_list_logic_and_titles_only_on_macos() {
        let logic = vec![RawWindow { hwnd: 1, pid: 300, exe: "Logic Pro".into(), title: String::new(), visible: true, ..Default::default() }];
        let r = Running::of(&logic, false, false);
        assert!(!r.live);
        let mut v = serde_json::json!({ "type": "windows", "windows": [] });
        r.put(&mut v);
        assert_eq!(v["live"], false);
        if cfg!(target_os = "macos") {
            assert_eq!((r.logic, r.titles, r.access), (Some(true), Some(false), Some(false)));
            assert_eq!((v["logic"].clone(), v["titles"].clone(), v["access"].clone()), (serde_json::json!(true), serde_json::json!(false), serde_json::json!(false)));
            assert_eq!(Running::of(&fake_desktop(), true, true), Running { live: true, logic: Some(false), titles: Some(true), access: Some(true) });
        } else {
            // Windows : rien sur Logic, les titres ni l'Accessibilite (le studio ne parle que de Live)
            assert_eq!((r.logic, r.titles, r.access), (None, None, None));
            assert!(v.get("logic").is_none() && v.get("titles").is_none() && v.get("access").is_none());
        }
    }

    fn fake_desktop() -> Vec<RawWindow> {
        vec![
            win(1, 100, LIVE, "Set 20 oct [Set 20 oct] - Ableton Live 12 Suite", 0, false), // la fenetre principale
            win(2, 100, LIVE, "Diva/2-Diva", 1, true),
            win(3, 100, LIVE, "Kontakt 8/1-Kontakt 8", 1, true),
            win(4, 100, LIVE, "Serum 2/3-Lead / Pad", 1, true), // une piste avec un « / »
            win(5, 100, LIVE, "Preferences", 1, true),          // pas « plugin/piste »
            win(6, 100, LIVE, "Diva/2-Diva", 1, false),         // pas une fenetre d'outil
            win(7, 200, "chrome.exe", "Diva/2-Diva - Google Chrome", 0, false),
            win(8, 200, "chrome.exe", "Diva/2-Diva", 7, true), // un autre processus qui imite le titre
            win(9, 100, LIVE, "Valhalla/4-Vox", 7, true),      // possedee par une fenetre d'un autre processus
            {
                let mut w = win(10, 100, LIVE, "Pro-Q 4/5-Master", 1, true);
                w.visible = false; // fermee (cachee) par Live
                w
            },
            win(11, 100, LIVE, "Pigments/", 1, true), // titre incomplet
        ]
    }

    #[test]
    fn finds_live_plugin_windows_only() {
        let got = plugin_windows_win(&fake_desktop());
        let ids: Vec<u64> = got.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![2, 3, 4], "{got:?}");
        assert_eq!(got[0].plugin, "Diva");
        assert_eq!(got[0].track, "2-Diva");
        assert_eq!(got[2].plugin, "Serum 2");
        assert_eq!(got[2].track, "3-Lead / Pad", "split at the first slash");
        assert_eq!((got[0].w, got[0].h), (800, 500));
    }

    #[test]
    fn no_live_no_windows() {
        let only_chrome: Vec<RawWindow> = fake_desktop().into_iter().filter(|w| w.pid == 200).collect();
        assert!(plugin_windows_win(&only_chrome).is_empty());
        assert!(!live_running(&only_chrome));
        assert!(live_running(&fake_desktop()));
    }

    #[test]
    fn live_exe_names() {
        assert!(is_live_exe("Ableton Live 12 Suite.exe"));
        assert!(is_live_exe("C:\\ProgramData\\Ableton\\Live 12 Suite\\Program\\Ableton Live 12 Suite.exe"));
        assert!(is_live_exe("ableton live 11 standard.EXE"));
        assert!(!is_live_exe("Ableton Index.exe"));
        assert!(!is_live_exe("chrome.exe"));
        assert!(!is_live_exe(""));
    }

    #[test]
    fn titles() {
        assert_eq!(split_title("Diva/2-Diva"), Some(("Diva".into(), "2-Diva".into())));
        assert_eq!(split_title("Diva"), None);
        assert_eq!(split_title("/2-Diva"), None);
        assert_eq!(expected_title("Diva", "2-Diva"), "Diva/2-Diva");
    }

    #[test]
    fn order_is_stable() {
        let mut d = fake_desktop();
        d.reverse();
        let ids: Vec<u64> = plugin_windows_win(&d).iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![2, 3, 4]);
    }
}

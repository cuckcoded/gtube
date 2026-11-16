//! Subtitles in the video player: which track to show, and the menu to pick
//! one (uploaded tracks, auto-generated ones and YouTube's translations).

use crate::extract::Captions;
use gtk::gio;

/// What the player shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Off,
    Track(usize),
    /// YouTube's automatic translation of a track.
    Translate { from: usize, to: String },
}

/// The track to load for a choice.
pub struct Source {
    pub url: String,
    /// mpv's name for the track.
    pub title: String,
    pub lang: String,
}

impl Choice {
    /// The menu's target for this choice.
    pub fn key(&self) -> String {
        match self {
            Choice::Off => "off".into(),
            Choice::Track(i) => format!("t{i}"),
            Choice::Translate { to, .. } => format!("x:{to}"),
        }
    }

    /// The choice a menu target stands for. Translations are of the track
    /// being shown, or else of the one YouTube would translate.
    pub fn from_key(key: &str, c: &Captions, current: &Choice) -> Option<Choice> {
        if key == "off" {
            return Some(Choice::Off);
        }
        if let Some(to) = key.strip_prefix("x:") {
            let shown = match current {
                Choice::Track(i) | Choice::Translate { from: i, .. } => Some(*i),
                Choice::Off => None,
            };
            let from = shown.filter(|i| *i < c.tracks.len()).or_else(|| translation_source(c))?;
            return c.translations.iter().any(|(code, _)| code == to).then(|| Choice::Translate { from, to: to.to_string() });
        }
        let i: usize = key.strip_prefix('t')?.parse().ok()?;
        (i < c.tracks.len()).then_some(Choice::Track(i))
    }

    pub fn source(&self, c: &Captions) -> Option<Source> {
        match self {
            Choice::Off => None,
            Choice::Track(i) => {
                let t = c.tracks.get(*i)?;
                Some(Source { url: t.vtt_url(None), title: t.name.clone(), lang: t.lang.clone() })
            }
            Choice::Translate { from, to } => {
                let t = c.tracks.get(*from)?;
                let name = c.translations.iter().find(|(code, _)| code == to).map_or(to.as_str(), |(_, n)| n.as_str());
                Some(Source { url: t.vtt_url(Some(to)), title: format!("{name} (auto-translated)"), lang: to.clone() })
            }
        }
    }
}

/// The primary language subtag, in lowercase ("pt-BR" → "pt").
fn primary(code: &str) -> String {
    code.split(['-', '_']).next().unwrap_or(code).to_lowercase()
}

/// The video's own subtitles, which are also what gets translated: uploaded
/// ones (made by people, so better to translate) in the language that is
/// spoken (that of the auto-generated track), else in English, else any,
/// else the auto-generated ones.
pub fn translation_source(c: &Captions) -> Option<usize> {
    let uploaded = |lang: &str| c.tracks.iter().position(|t| !t.auto && primary(&t.lang) == lang);
    let asr = c.tracks.iter().position(|t| t.auto);
    asr.and_then(|a| uploaded(&primary(&c.tracks[a].lang)))
        .or_else(|| uploaded("en"))
        .or_else(|| c.tracks.iter().position(|t| !t.auto))
        .or(asr)
}

/// What to show someone who wants subtitles in `lang`: a track in that
/// language (an uploaded one first), else its translation if `translate`,
/// else the video's own subtitles, as YouTube does.
pub fn preferred(c: &Captions, lang: &str, translate: bool) -> Choice {
    let Some(default) = translation_source(c) else { return Choice::Off };
    let want = primary(lang);
    let exact = |auto: bool| c.tracks.iter().position(|t| t.auto == auto && t.lang.eq_ignore_ascii_case(lang));
    let close = |auto: bool| c.tracks.iter().position(|t| t.auto == auto && primary(&t.lang) == want);
    if let Some(i) = exact(false).or_else(|| close(false)).or_else(|| exact(true)).or_else(|| close(true)) {
        return Choice::Track(i);
    }
    if translate {
        let code = c
            .translations
            .iter()
            .find(|(code, _)| code.eq_ignore_ascii_case(lang))
            .or_else(|| c.translations.iter().find(|(code, _)| primary(code) == want));
        if let Some((code, _)) = code {
            return Choice::Translate { from: default, to: code.clone() };
        }
    }
    Choice::Track(default)
}

/// Menu labels underline the letter after "_".
fn label(s: &str) -> String {
    s.replace('_', "__")
}

/// The subtitles menu (items activate `player.subtitles`).
pub fn menu(c: &Captions) -> gio::Menu {
    let menu = gio::Menu::new();
    let off = gio::Menu::new();
    off.append(Some("Off"), Some(&format!("player.subtitles::{}", Choice::Off.key())));
    menu.append_section(None, &off);
    let tracks = gio::Menu::new();
    for (i, t) in c.tracks.iter().enumerate() {
        tracks.append(Some(&label(&t.name)), Some(&format!("player.subtitles::{}", Choice::Track(i).key())));
    }
    menu.append_section(None, &tracks);
    if !c.tracks.is_empty() && !c.translations.is_empty() {
        let languages = gio::Menu::new();
        for (code, name) in &c.translations {
            languages.append(Some(&label(name)), Some(&format!("player.subtitles::x:{code}")));
        }
        let section = gio::Menu::new();
        section.append_submenu(Some("Auto-translate"), &languages);
        menu.append_section(None, &section);
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::CaptionTrack;

    fn captions(tracks: &[(&str, bool)], translations: &[&str]) -> Captions {
        Captions {
            tracks: tracks
                .iter()
                .map(|(lang, auto)| CaptionTrack {
                    lang: lang.to_string(),
                    name: lang.to_string(),
                    auto: *auto,
                    url: format!("https://www.youtube.com/api/timedtext?lang={lang}"),
                })
                .collect(),
            translations: translations.iter().map(|t| (t.to_string(), t.to_string())).collect(),
        }
    }

    #[test]
    fn prefers_uploaded_tracks_in_the_language() {
        let c = captions(&[("de-DE", false), ("en-GB", false), ("en", true)], &["it", "pt-BR"]);
        assert_eq!(preferred(&c, "en", false), Choice::Track(1));
        assert_eq!(preferred(&c, "de", true), Choice::Track(0));
        // Nothing in Italian: the uploaded English track (English is spoken)
        // or its translation.
        assert_eq!(preferred(&c, "it", false), Choice::Track(1));
        assert_eq!(preferred(&c, "it", true), Choice::Translate { from: 1, to: "it".into() });
        assert_eq!(preferred(&c, "pt", true), Choice::Translate { from: 1, to: "pt-BR".into() });
        assert_eq!(preferred(&c, "fr", true), Choice::Track(1));
        assert_eq!(preferred(&captions(&[("ja", true)], &["en"]), "en", false), Choice::Track(0));
        assert_eq!(preferred(&captions(&[], &[]), "en", true), Choice::Off);
    }

    #[test]
    fn menu_targets_round_trip() {
        let c = captions(&[("en", false), ("ja", true)], &["it"]);
        for choice in [Choice::Off, Choice::Track(1)] {
            assert_eq!(Choice::from_key(&choice.key(), &c, &Choice::Off), Some(choice));
        }
        // Translations are of what is shown, or of the default track.
        assert_eq!(Choice::from_key("x:it", &c, &Choice::Track(1)), Some(Choice::Translate { from: 1, to: "it".into() }));
        assert_eq!(Choice::from_key("x:it", &c, &Choice::Off), Some(Choice::Translate { from: 0, to: "it".into() }));
        assert_eq!(Choice::from_key("x:fr", &c, &Choice::Off), None);
        assert_eq!(Choice::from_key("t5", &c, &Choice::Off), None);
        let s = Choice::Translate { from: 1, to: "it".into() }.source(&c).unwrap();
        assert_eq!(s.url, "https://www.youtube.com/api/timedtext?lang=ja&fmt=vtt&tlang=it");
        assert_eq!((s.title.as_str(), s.lang.as_str()), ("it (auto-translated)", "it"));
    }
}

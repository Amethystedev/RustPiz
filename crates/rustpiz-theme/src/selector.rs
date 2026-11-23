//! Sélecteurs XAML, à la syntaxe du Taskbar Styler.
//!
//! Forme : `Type#Nom@GroupeEtats[Prop=Val][1] > Enfant`, avec `*` pour un
//! nombre quelconque d'ancêtres et `:root` pour la racine de l'arbre.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SelectorError(pub String);

fn err<T>(message: &str) -> Result<T, SelectorError> {
    Err(SelectorError(message.to_owned()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementMatcher {
    /// Nom de classe complet, abréviations résolues.
    pub type_name: String,
    pub name: Option<String>,
    pub state_group: Option<String>,
    /// Position 1-based parmi les enfants du parent, 0 si absente.
    pub index: u32,
    pub properties: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Matcher {
    Element(ElementMatcher),
    Wildcard,
    Root,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    /// Texte d'origine, repris dans les journaux.
    pub raw: String,
    pub leaf: ElementMatcher,
    /// Ancêtres, du parent direct vers la racine.
    pub ancestors: Vec<Matcher>,
}

/// Résout les abréviations de type, comme `AdjustTypeName` dans le Styler.
pub fn adjust_type_name(name: &str) -> String {
    if !name.contains(['.', ':']) {
        return if name == "Rectangle" {
            "Windows.UI.Xaml.Shapes.Rectangle".to_owned()
        } else {
            format!("Windows.UI.Xaml.Controls.{name}")
        };
    }
    const PREFIXES: [(&str, &str); 4] = [
        ("taskbar:", "Taskbar."),
        ("systemtray:", "SystemTray."),
        ("udk:", "WindowsUdk.UI.Shell."),
        ("muxc:", "Microsoft.UI.Xaml.Controls."),
    ];
    for (short, long) in PREFIXES {
        if let Some(rest) = name.strip_prefix(short) {
            return format!("{long}{rest}");
        }
    }
    name.to_owned()
}

/// Découpe sur `separator` en ignorant le contenu des crochets.
fn split_outside_brackets<'a>(text: &'a str, separator: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut pos = 0;
    while pos < text.len() {
        let rest = &text[pos..];
        if depth == 0 && rest.starts_with(separator) {
            parts.push(&text[start..pos]);
            pos += separator.len();
            start = pos;
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        pos += ch.len_utf8();
    }
    parts.push(&text[start..]);
    parts
}

fn parse_part(part: &str) -> Result<Matcher, SelectorError> {
    let trimmed = part.trim();
    match trimmed {
        "*" => return Ok(Matcher::Wildcard),
        ":root" => return Ok(Matcher::Root),
        _ => {}
    }

    let first_marker = trimmed.find(['#', '@', '[']).unwrap_or(trimmed.len());
    let type_name = trimmed[..first_marker].trim();
    if type_name.is_empty() {
        return err("type d'élément vide");
    }
    let mut matcher = ElementMatcher {
        type_name: adjust_type_name(type_name),
        ..ElementMatcher::default()
    };

    let mut rest = &trimmed[first_marker..];
    while let Some(marker) = rest.chars().next() {
        match marker {
            '[' => {
                let Some(close) = rest.find(']') else {
                    return err("']' manquant");
                };
                let inner = rest[1..close].trim();
                if inner.is_empty() {
                    return err("propriété vide entre crochets");
                }
                if inner.bytes().all(|b| b.is_ascii_digit()) {
                    matcher.index = inner
                        .parse()
                        .map_err(|_| SelectorError("index trop grand".to_owned()))?;
                } else {
                    let Some((key, value)) = inner.split_once('=') else {
                        return err("'=' manquant dans la propriété entre crochets");
                    };
                    if key.trim().is_empty() {
                        return err("nom de propriété vide entre crochets");
                    }
                    matcher
                        .properties
                        .push((key.trim().to_owned(), value.trim().to_owned()));
                }
                rest = &rest[close + 1..];
            }
            '#' | '@' => {
                let body = &rest[1..];
                let end = body.find(['#', '@', '[']).unwrap_or(body.len());
                let value = body[..end].trim();
                if marker == '#' {
                    if matcher.name.is_some() {
                        return err("plusieurs noms sur un même élément");
                    }
                    if value.is_empty() {
                        return err("nom d'élément vide");
                    }
                    matcher.name = Some(value.to_owned());
                } else {
                    if matcher.state_group.is_some() {
                        return err("plusieurs groupes d'états sur un même élément");
                    }
                    matcher.state_group = Some(value.to_owned());
                }
                rest = &body[end..];
            }
            c if c.is_whitespace() => rest = rest.trim_start(),
            _ => return err("texte inattendu après un crochet"),
        }
    }
    Ok(Matcher::Element(matcher))
}

/// Analyse une cible simple, sans virgule de premier niveau.
pub fn parse_selector(target: &str) -> Result<Selector, SelectorError> {
    let mut parts = split_outside_brackets(target, " > ");
    let Some(leaf_text) = parts.pop() else {
        return err("cible vide");
    };
    let Matcher::Element(leaf) = parse_part(leaf_text)? else {
        return err("'*' et ':root' ne peuvent pas être l'élément ciblé");
    };

    let mut ancestors = Vec::with_capacity(parts.len());
    let mut groups = usize::from(leaf.state_group.is_some());
    for (position, part) in parts.iter().enumerate().rev() {
        let leftmost = position == 0;
        let matcher = parse_part(part)?;
        let previous_is_wildcard = matches!(ancestors.last(), Some(Matcher::Wildcard));
        match &matcher {
            Matcher::Wildcard if leftmost => return err("'*' ne peut pas être tout à gauche"),
            Matcher::Wildcard if previous_is_wildcard => return err("deux '*' adjacents"),
            Matcher::Root if !leftmost => return err("':root' doit être tout à gauche"),
            Matcher::Root if previous_is_wildcard => return err("':root' ne peut pas suivre '*'"),
            Matcher::Element(element) if element.state_group.is_some() => groups += 1,
            _ => {}
        }
        ancestors.push(matcher);
    }
    if groups > 1 {
        return err("une cible ne peut référencer qu'un seul groupe d'états");
    }
    Ok(Selector {
        raw: target.trim().to_owned(),
        leaf,
        ancestors,
    })
}

/// Analyse une chaîne `target` pouvant contenir plusieurs cibles.
pub fn parse_targets(target: &str) -> Vec<Result<Selector, (String, SelectorError)>> {
    split_outside_brackets(target, ",")
        .into_iter()
        .map(|single| parse_selector(single).map_err(|e| (single.trim().to_owned(), e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_syntax_is_parsed() {
        let s = parse_selector(
            "Taskbar.ExperienceToggleButton#LaunchListButton[AutomationProperties.AutomationId=StartButton]",
        )
        .unwrap();
        assert_eq!(s.leaf.type_name, "Taskbar.ExperienceToggleButton");
        assert_eq!(s.leaf.name.as_deref(), Some("LaunchListButton"));
        assert_eq!(s.leaf.properties[0].0, "AutomationProperties.AutomationId");
    }

    #[test]
    fn hierarchy_goes_from_parent_to_root() {
        let s = parse_selector("Grid#A@G > * > ContentPresenter[2] > Rectangle#R").unwrap();
        assert_eq!(s.leaf.type_name, "Windows.UI.Xaml.Shapes.Rectangle");
        assert_eq!(s.ancestors.len(), 3);
        assert!(
            matches!(&s.ancestors[0], Matcher::Element(e) if e.index == 2
                && e.type_name == "Windows.UI.Xaml.Controls.ContentPresenter")
        );
        assert_eq!(s.ancestors[1], Matcher::Wildcard);
        assert!(matches!(&s.ancestors[2], Matcher::Element(e)
            if e.state_group.as_deref() == Some("G")));
    }

    #[test]
    fn prefixes_are_expanded() {
        assert_eq!(adjust_type_name("taskbar:Foo"), "Taskbar.Foo");
        assert_eq!(
            adjust_type_name("muxc:Bar"),
            "Microsoft.UI.Xaml.Controls.Bar"
        );
        assert_eq!(adjust_type_name("A.B"), "A.B");
    }

    #[test]
    fn commas_inside_brackets_do_not_split_targets() {
        let parsed = parse_targets("Grid[Margin=1,2,3,4], Border");
        assert_eq!(parsed.len(), 2);
        assert!(parsed.iter().all(Result::is_ok));
    }

    #[test]
    fn invalid_forms_are_rejected() {
        for bad in [
            "",
            "*",
            ":root",
            "* > Grid",
            "Grid > :root > Border",
            "A > * > * > B",
            "Grid#A#B",
            "Grid[x",
            "Grid[]",
            "A@X > B@Y",
            ":root > * > A",
            "#Name",
        ] {
            assert!(parse_selector(bad).is_err(), "devrait échouer : {bad:?}");
        }
    }

    #[test]
    fn root_is_accepted_on_the_left() {
        let s = parse_selector(":root > Grid > Border").unwrap();
        assert_eq!(s.ancestors.last(), Some(&Matcher::Root));
    }
}

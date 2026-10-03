//! A .dosc's launch configurations, its `[...]` folders, as the player
//! chooses among them: other ways of starting the game (`[Setup
//! Program]`), and settings to combine (`[MIDI + German]`), each part of
//! such a name one of the options of a category. The combination no
//! folder has is the default, the .dosc's root. A `n#` or `nn#` before a
//! name or part orders it and isn't shown.

/// A launch configuration of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its folder's name, without the brackets.
    pub dir: String,
    /// Its name as shown.
    pub name: String,
}

/// The launch configurations, ready to choose from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Variants {
    /// The options of each category, as shown, the default's first.
    pub categories: Vec<Vec<String>>,
    /// The folders of the combinations: the option chosen in each
    /// category, and the folder.
    combinations: Vec<(Vec<usize>, String)>,
    /// The launch configurations that aren't combinations.
    pub entries: Vec<Entry>,
}

/// A name or part with its order: the number before `#`, if any.
fn ordered(part: &str) -> (u32, String) {
    let part = part.trim();
    match part.split_once('#') {
        Some((n, rest)) if (1..=2).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()) => {
            (n.parse().unwrap_or(u32::MAX), rest.trim().to_string())
        }
        _ => (u32::MAX, part.to_string()),
    }
}

impl Variants {
    /// A folder's name as shown: without its order numbers.
    pub fn shown(dir: &str) -> String {
        dir.split('+').map(|p| ordered(p).1).collect::<Vec<_>>().join(" + ")
    }

    /// The launch configurations of the folders `dirs` (names without
    /// brackets).
    pub fn new(dirs: &[String]) -> Self {
        let mut variants = Self::default();
        let (combined, plain): (Vec<&String>, Vec<&String>) = dirs.iter().partition(|d| d.contains('+'));
        let mut entries: Vec<((u32, String), Entry)> = plain
            .into_iter()
            .map(|dir| {
                let (order, name) = ordered(dir);
                ((order, name.to_lowercase()), Entry { dir: dir.clone(), name })
            })
            .collect();
        entries.sort_by_key(|(order, _)| order.clone());
        variants.entries = entries.into_iter().map(|(_, e)| e).collect();

        // Each category's options, in order.
        let count = combined.iter().map(|d| d.split('+').count()).max().unwrap_or(0);
        let mut options: Vec<Vec<(u32, String)>> = vec![Vec::new(); count];
        for dir in &combined {
            for (k, part) in dir.split('+').enumerate() {
                let (order, name) = ordered(part);
                if !options[k].iter().any(|(_, n)| n.eq_ignore_ascii_case(&name)) {
                    options[k].push((order, name));
                }
            }
        }
        for list in &mut options {
            list.sort_by_key(|(order, name)| (*order, name.to_lowercase()));
        }
        let index = |k: usize, part: &str| {
            let name = ordered(part).1;
            options[k].iter().position(|(_, n)| n.eq_ignore_ascii_case(&name)).unwrap_or(0)
        };
        let mut combinations: Vec<(Vec<usize>, String)> = combined
            .iter()
            .map(|dir| {
                let mut picks: Vec<usize> = dir.split('+').enumerate().map(|(k, p)| index(k, p)).collect();
                picks.resize(count, 0);
                (picks, (*dir).clone())
            })
            .collect();
        // The default: the first combination no folder has, its options
        // put first.
        let total: usize = options.iter().map(Vec::len).product();
        let default = (0..total).map(|n| {
            let mut picks = Vec::with_capacity(count);
            let mut n = n;
            for list in &options {
                picks.push(n % list.len());
                n /= list.len();
            }
            picks
        });
        let default = default.into_iter().find(|picks| !combinations.iter().any(|(c, _)| c == picks));
        if let Some(default) = default {
            for (k, &first) in default.iter().enumerate() {
                let option = options[k].remove(first);
                options[k].insert(0, option);
                for (picks, _) in &mut combinations {
                    picks[k] = match picks[k] {
                        p if p == first => 0,
                        p if p < first => p + 1,
                        p => p,
                    };
                }
            }
        }
        variants.categories = options.into_iter().map(|list| list.into_iter().map(|(_, n)| n).collect()).collect();
        variants.combinations = combinations;
        variants
    }

    pub fn is_empty(&self) -> bool {
        self.categories.is_empty() && self.entries.is_empty()
    }

    /// The folder of the options `picks` (one for each category), or None
    /// for the default.
    pub fn combination(&self, picks: &[usize]) -> Option<String> {
        self.combinations.iter().find(|(c, _)| c == picks).map(|(_, dir)| dir.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn plain_launch_configurations_are_in_order() {
        let v = Variants::new(&dirs(&["Ship Editor", "Setup Program", "General MIDI Music", "1#Play Online"]));
        let names: Vec<&str> = v.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Play Online", "General MIDI Music", "Setup Program", "Ship Editor"]);
        assert_eq!(v.entries[0].dir, "1#Play Online");
        assert!(v.categories.is_empty());
    }

    #[test]
    fn categories_have_the_default_first() {
        // The format's example: Adlib + English is the root's.
        let v = Variants::new(&dirs(&["Adlib + German", "MIDI + English", "MIDI + German", "Setup"]));
        assert_eq!(v.categories, [vec!["Adlib", "MIDI"], vec!["English", "German"]]);
        assert_eq!(v.combination(&[0, 0]), None, "the default");
        assert_eq!(v.combination(&[0, 1]).as_deref(), Some("Adlib + German"));
        assert_eq!(v.combination(&[1, 0]).as_deref(), Some("MIDI + English"));
        assert_eq!(v.entries.len(), 1);
        // Ordered by their numbers, which aren't shown.
        let v = Variants::new(&dirs(&["2#Adlib + 3#CGA", "1#MIDI + 3#CGA", "1#MIDI + 1#VGA"]));
        assert_eq!(v.categories, [vec!["Adlib", "MIDI"], vec!["VGA", "CGA"]]);
        assert_eq!(Variants::shown("2#Adlib + 3#CGA"), "Adlib + CGA");
    }
}

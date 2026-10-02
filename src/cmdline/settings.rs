//! The `[shell]` settings: the prompt's suggestions, colours and history.

/// A colour of the text screen's 16, or none of its own: the screen's
/// where the text goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DosColor(pub Option<u8>);

const COLOR_NAMES: [&str; 16] = [
    "black", "blue", "green", "cyan", "red", "magenta", "brown", "lightgray", "darkgray", "lightblue", "lightgreen",
    "lightcyan", "lightred", "lightmagenta", "yellow", "white",
];

impl DosColor {
    pub fn parse(value: &str) -> Option<Self> {
        let name: String = value.trim().to_ascii_lowercase().chars().filter(|c| !matches!(c, ' ' | '_' | '-')).collect();
        let name = name.replace("grey", "gray");
        match name.as_str() {
            "default" | "none" | "" => Some(DosColor(None)),
            "gray" => Some(DosColor(Some(7))),
            _ => COLOR_NAMES.iter().position(|n| *n == name).map(|i| DosColor(Some(i as u8))),
        }
    }

    pub fn name(self) -> &'static str {
        self.0.map_or("default", |c| COLOR_NAMES[c as usize & 15])
    }

    /// The attribute of text in this colour on a screen of attribute
    /// `screen`: its background, and this colour or its foreground.
    pub fn on(self, screen: u8) -> u8 {
        match self.0 {
            Some(c) => (screen & 0xF0) | c,
            None => screen,
        }
    }
}

/// The colours of the prompt and the parts of the line typed at it, as
/// clink's color.* settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub prompt: DosColor,
    /// A built-in command.
    pub command: DosColor,
    /// A program found, or a drive to change to.
    pub executable: DosColor,
    /// A command that is neither.
    pub unrecognized: DosColor,
    pub argument: DosColor,
    /// A switch, /X.
    pub flag: DosColor,
    /// The rest of the line suggested after the cursor.
    pub suggestion: DosColor,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            prompt: DosColor(Some(7)),
            command: DosColor(Some(15)),
            executable: DosColor(Some(11)),
            unrecognized: DosColor(Some(12)),
            argument: DosColor(None),
            flag: DosColor(Some(14)),
            suggestion: DosColor(Some(8)),
        }
    }
}

/// The `[shell]` settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellSettings {
    /// The rest of a line from the history shown after the cursor.
    pub autosuggest: bool,
    /// The prompt and the line in the palette's colours.
    pub colors: bool,
    /// The history kept in a file between sessions.
    pub save_history: bool,
    /// The lines the history keeps.
    pub history_size: usize,
    pub palette: Palette,
}

impl Default for ShellSettings {
    fn default() -> Self {
        ShellSettings {
            autosuggest: true,
            colors: true,
            save_history: true,
            history_size: super::history::DEFAULT_SIZE,
            palette: Palette::default(),
        }
    }
}

impl ShellSettings {
    /// The colour setting called `key`.
    fn color(&mut self, key: &str) -> Option<&mut DosColor> {
        let p = &mut self.palette;
        Some(match key {
            "prompt_color" => &mut p.prompt,
            "command_color" => &mut p.command,
            "executable_color" => &mut p.executable,
            "unrecognized_color" => &mut p.unrecognized,
            "argument_color" => &mut p.argument,
            "flag_color" => &mut p.flag,
            "suggestion_color" => &mut p.suggestion,
            _ => return None,
        })
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bool_of = |v: &str| match v.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            _ => Err(format!("invalid {} '{}' (true or false)", key, v)),
        };
        let key = key.to_ascii_lowercase();
        match key.as_str() {
            "autosuggest" => self.autosuggest = bool_of(value)?,
            "colors" => self.colors = bool_of(value)?,
            "save_history" => self.save_history = bool_of(value)?,
            "history_size" => match value.trim().parse::<usize>() {
                Ok(n) if (1..=100_000).contains(&n) => self.history_size = n,
                _ => return Err(format!("invalid history_size '{}' (1 to 100000)", value)),
            },
            _ => {
                let Some(color) = self.color(&key) else { return Err(format!("unknown setting '{}'", key)) };
                *color = DosColor::parse(value)
                    .ok_or_else(|| format!("invalid {} '{}' (a colour such as lightgreen, or default)", key, value))?;
            }
        }
        Ok(())
    }

    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        let p = &self.palette;
        let mut entries = vec![
            ("autosuggest", Some(self.autosuggest.to_string())),
            ("colors", Some(self.colors.to_string())),
            ("save_history", Some(self.save_history.to_string())),
            ("history_size", Some(self.history_size.to_string())),
        ];
        entries.extend([
            ("prompt_color", p.prompt),
            ("command_color", p.command),
            ("executable_color", p.executable),
            ("unrecognized_color", p.unrecognized),
            ("argument_color", p.argument),
            ("flag_color", p.flag),
            ("suggestion_color", p.suggestion),
        ]
        .map(|(key, color)| (key, Some(color.name().to_string()))));
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let mut s = ShellSettings::default();
        s.set("Prompt_Color", "Light Grey").unwrap();
        s.set("flag_color", "default").unwrap();
        s.set("history_size", "50").unwrap();
        assert_eq!(s.palette.prompt, DosColor(Some(7)));
        assert!(s.set("command_color", "plaid").is_err());
        assert!(s.set("history_size", "0").is_err());
        let mut again = ShellSettings::default();
        for (key, value) in s.entries() {
            again.set(key, &value.unwrap()).unwrap();
        }
        assert_eq!(again, s);
    }

    #[test]
    fn a_colour_keeps_the_screens_background() {
        assert_eq!(DosColor(Some(14)).on(0x17), 0x1E);
        assert_eq!(DosColor(None).on(0x17), 0x17);
    }
}

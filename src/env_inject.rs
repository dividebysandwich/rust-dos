//! Variables the settings keep in the guest's environment, unless the guest
//! has set them itself.

use crate::cpu::Cpu;

/// A variable a setting wants: its name, and its value, or None for none.
pub type Rule = (&'static str, Option<String>);

/// The guest's environment, as the injector changes it.
pub trait GuestEnvironment {
    fn get(&self, name: &str) -> Option<&str>;
    /// Set a variable, or remove it for an empty value; false when it doesn't fit.
    fn set(&mut self, name: &str, value: &str) -> bool;
}

impl GuestEnvironment for Cpu {
    fn get(&self, name: &str) -> Option<&str> {
        self.get_env(name)
    }

    fn set(&mut self, name: &str, value: &str) -> bool {
        self.set_env(name, value)
    }
}

/// Brings the rules' variables into the environment. A variable the guest
/// sets or changes is the guest's from then on.
#[derive(Debug, Default)]
pub struct EnvInjector {
    rules: Vec<Rule>,
    /// The variables injected, with the values they were given.
    injected: Vec<(String, String)>,
}

impl EnvInjector {
    pub fn set_rules(&mut self, rules: Vec<Rule>) {
        self.rules = rules;
    }

    /// Bring the environment in line with the rules; false if a variable didn't fit.
    pub fn apply<E: GuestEnvironment + ?Sized>(&mut self, env: &mut E) -> bool {
        self.injected.retain(|(name, value)| env.get(name) == Some(value.as_str()));
        let stale: Vec<String> = self
            .injected
            .iter()
            .filter(|(name, _)| !self.rules.iter().any(|(rule, _)| rule.eq_ignore_ascii_case(name)))
            .map(|(name, _)| name.clone())
            .collect();
        let mut fit = true;
        for name in stale {
            fit &= self.clear(env, &name);
        }
        for (name, wanted) in self.rules.clone() {
            fit &= match wanted {
                Some(value) => self.inject(env, name, &value),
                None => self.clear(env, name),
            };
        }
        fit
    }

    fn inject<E: GuestEnvironment + ?Sized>(&mut self, env: &mut E, name: &str, value: &str) -> bool {
        match self.injected.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some(i) => {
                if self.injected[i].1 == value {
                    return true;
                }
                if !env.set(name, value) {
                    return false;
                }
                self.injected[i].1 = value.to_string();
                true
            }
            None if env.get(name).is_some() => true,
            None => {
                if !env.set(name, value) {
                    return false;
                }
                self.injected.push((name.to_ascii_uppercase(), value.to_string()));
                true
            }
        }
    }

    fn clear<E: GuestEnvironment + ?Sized>(&mut self, env: &mut E, name: &str) -> bool {
        match self.injected.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some(i) => {
                self.injected.remove(i);
                env.set(name, "")
            }
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Env {
        vars: Vec<(String, String)>,
        full: bool,
    }

    impl GuestEnvironment for Env {
        fn get(&self, name: &str) -> Option<&str> {
            self.vars.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }

        fn set(&mut self, name: &str, value: &str) -> bool {
            if self.full && !value.is_empty() {
                return false;
            }
            let name = name.to_ascii_uppercase();
            self.vars.retain(|(n, _)| *n != name);
            if !value.is_empty() {
                self.vars.push((name, value.to_string()));
            }
            true
        }
    }

    const GAMMAS: [&str; 3] = ["SST_RGAMMA", "SST_GGAMMA", "SST_BGAMMA"];

    fn gamma(value: Option<&str>) -> Vec<Rule> {
        GAMMAS.map(|name| (name, value.map(str::to_string))).to_vec()
    }

    fn injector(value: Option<&str>) -> EnvInjector {
        let mut injector = EnvInjector::default();
        injector.set_rules(gamma(value));
        injector
    }

    #[test]
    fn absent_variables_are_injected() {
        let mut env = Env::default();
        assert!(injector(Some("1.5")).apply(&mut env));
        for name in GAMMAS {
            assert_eq!(env.get(name), Some("1.5"));
        }
    }

    #[test]
    fn variables_the_guest_set_are_kept() {
        let mut env = Env::default();
        env.set("SST_GGAMMA", "2");
        injector(Some("1.5")).apply(&mut env);
        assert_eq!(env.get("SST_GGAMMA"), Some("2"));
        assert_eq!(env.get("SST_RGAMMA"), Some("1.5"));
    }

    #[test]
    fn a_value_the_guest_changes_is_theirs() {
        let mut env = Env::default();
        let mut injector = injector(Some("1.5"));
        injector.apply(&mut env);
        env.set("SST_RGAMMA", "2");
        injector.apply(&mut env);
        assert_eq!(env.get("SST_RGAMMA"), Some("2"));
    }

    #[test]
    fn a_variable_the_guest_removed_comes_back() {
        let mut env = Env::default();
        let mut injector = injector(Some("1.5"));
        injector.apply(&mut env);
        env.set("SST_RGAMMA", "");
        injector.apply(&mut env);
        assert_eq!(env.get("SST_RGAMMA"), Some("1.5"));
    }

    #[test]
    fn a_changed_setting_updates_its_variables() {
        let mut env = Env::default();
        let mut injector = injector(Some("1.5"));
        injector.apply(&mut env);
        injector.set_rules(gamma(Some("2")));
        injector.apply(&mut env);
        assert_eq!(env.get("SST_BGAMMA"), Some("2"));
    }

    #[test]
    fn turning_off_removes_only_what_was_injected() {
        let mut env = Env::default();
        env.set("SST_BGAMMA", "9");
        let mut injector = injector(Some("1.5"));
        injector.apply(&mut env);
        injector.set_rules(gamma(None));
        injector.apply(&mut env);
        assert_eq!(env.get("SST_RGAMMA"), None);
        assert_eq!(env.get("SST_BGAMMA"), Some("9"));
    }

    #[test]
    fn a_rule_that_is_dropped_removes_its_variable() {
        let mut env = Env::default();
        let mut injector = injector(Some("1.5"));
        injector.apply(&mut env);
        injector.set_rules(Vec::new());
        injector.apply(&mut env);
        assert_eq!(env.get("SST_GGAMMA"), None);
    }

    #[test]
    fn reapplying_changes_nothing() {
        let mut env = Env::default();
        let mut injector = injector(Some("1.5"));
        injector.apply(&mut env);
        injector.apply(&mut env);
        assert_eq!(env.vars.len(), 3);
    }

    #[test]
    fn a_full_environment_takes_none() {
        let mut env = Env { full: true, ..Env::default() };
        assert!(!injector(Some("1.5")).apply(&mut env));
        assert!(env.vars.is_empty());
    }
}

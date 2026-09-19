//! Configured values, separated from the credentials among them.

use std::fmt::{Debug, Display, Formatter};

/// A credential-shaped configuration value.
///
/// The inner string is reachable only through [`SecretValue::expose`], which the
/// execution layer calls when building a process environment or a header map.
/// [`Debug`] and [`Display`] both render `<redacted>`, so a secret cannot reach a
/// log or a diagnostic through ordinary formatting.
///
/// This type deliberately implements neither `Serialize` nor `Deserialize`. That
/// is the enforcement mechanism for "no configured credential is ever written to
/// disk": adding a `Serialize` derive to any struct that transitively contains a
/// `SecretValue` is a compile error, not something a reviewer has to notice.
///
/// It does not zeroize on drop. Without a crate such as `zeroize` a manual
/// overwrite is legal for the compiler to elide, and claiming a guarantee the
/// code does not keep is worse than documenting the real behavior: the value
/// lives in process memory for the process lifetime, exactly as the environment
/// variable it came from does.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    /// Wraps a configured credential.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the credential for use in a subprocess environment or a header.
    ///
    /// Every call site is a place a credential leaves this crate, so the name is
    /// deliberately conspicuous.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Debug for SecretValue {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl Display for SecretValue {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// What one configured value contributes to a server's identity.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum EnvClass {
    /// Selects which server, account, or dataset is reached.
    ///
    /// Both the key and the value take part in identity, because two servers
    /// differing only here are genuinely different servers.
    Identity,
    /// A credential. The key takes part in identity; the value never does.
    Secret,
    /// Ambient shell state that carries no server identity and is excluded.
    Ambient,
}

/// One configured environment variable or header value, with its class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    /// A value that selects which server or account is reached.
    Identity(String),
    /// A credential, held opaque.
    Secret(SecretValue),
    /// Ambient shell state, kept for launch but excluded from identity.
    Ambient(String),
}

impl EnvValue {
    /// Classifies a configured value from its key alone.
    ///
    /// Classification never inspects the value. An entropy or shape heuristic
    /// over the value would reclassify a key when its token is rotated, which
    /// would change the server's identity and rename its namespace for no
    /// semantic reason.
    pub fn classify(key: &str, value: impl Into<String>) -> Self {
        match classify_key(key) {
            EnvClass::Identity => Self::Identity(value.into()),
            EnvClass::Secret => Self::Secret(SecretValue::new(value)),
            EnvClass::Ambient => Self::Ambient(value.into()),
        }
    }

    /// Returns this value's class.
    pub fn class(&self) -> EnvClass {
        match self {
            Self::Identity(_) => EnvClass::Identity,
            Self::Secret(_) => EnvClass::Secret,
            Self::Ambient(_) => EnvClass::Ambient,
        }
    }

    /// Returns the value for launching the server.
    pub fn expose(&self) -> &str {
        match self {
            Self::Identity(value) | Self::Ambient(value) => value,
            Self::Secret(secret) => secret.expose(),
        }
    }

    /// Returns the value only when it may take part in identity.
    pub fn identity_value(&self) -> Option<&str> {
        match self {
            Self::Identity(value) => Some(value),
            Self::Secret(_) | Self::Ambient(_) => None,
        }
    }
}

/// Final words that mark a key as naming a credential.
///
/// The match is on the key's last word, not on a substring anywhere in it.
/// Substring matching classifies `MAX_TOKENS` as a credential because it
/// contains `TOKEN`, which is wrong: that is a token *budget*, and excluding it
/// from identity would merge two servers that differ only by it.
const SECRET_FINAL_WORDS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "KEY",
    "APIKEY",
    "CREDENTIAL",
    "CREDENTIALS",
    "PAT",
    "COOKIE",
    "BEARER",
    "AUTH",
    "AUTHORIZATION",
];

/// Multi-word markers recognised anywhere in a key.
///
/// These name a credential even when another word follows, as in
/// `AWS_SECRET_ACCESS_KEY_ID`.
const SECRET_PHRASES: &[&str] = &[
    "API_KEY",
    "ACCESS_KEY",
    "PRIVATE_KEY",
    "SECRET_KEY",
    "CLIENT_SECRET",
    "ACCESS_TOKEN",
    "REFRESH_TOKEN",
    "WEBHOOK_SECRET",
];

/// Keys whose names resemble a credential but which hold an enumeration.
const SECRET_EXCEPTIONS: &[&str] = &["AUTH_MODE", "AUTH_TYPE", "AUTH_PROVIDER", "AUTHORITY"];

/// Suffixes that make a key name a location rather than a credential.
///
/// `GITHUB_TOKEN_FILE` names the file holding the token. The path selects which
/// account is used and is not itself a credential, so it belongs in identity.
const LOCATION_SUFFIXES: &[&str] = &["_FILE", "_PATH", "_DIR"];

/// Variables that describe the shell rather than the server.
const AMBIENT_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "TEMP",
    "TMP",
    "TERM",
    "COLORTERM",
    "LANG",
    "PWD",
    "OLDPWD",
    "SSH_AUTH_SOCK",
    "DISPLAY",
    "NO_COLOR",
    "FORCE_COLOR",
    "CI",
];

/// Classifies one configured key.
///
/// The order matters: ambient state first, then explicit exceptions, then a
/// location suffix, then credential shapes. A key reaching the end is treated as
/// identity-bearing, because a value that selects which server is reached is far
/// more common than an unrecognised credential.
pub fn classify_key(key: &str) -> EnvClass {
    let upper = key.to_ascii_uppercase();
    if AMBIENT_KEYS.contains(&upper.as_str()) || upper.starts_with("LC_") {
        return EnvClass::Ambient;
    }
    if SECRET_EXCEPTIONS.contains(&upper.as_str()) {
        return EnvClass::Identity;
    }
    if LOCATION_SUFFIXES
        .iter()
        .any(|suffix| upper.ends_with(suffix))
    {
        return EnvClass::Identity;
    }
    if SECRET_PHRASES.iter().any(|phrase| upper.contains(phrase)) {
        return EnvClass::Secret;
    }
    let last_word = upper
        .rsplit(|ch: char| !ch.is_ascii_alphanumeric())
        .find(|word| !word.is_empty())
        .unwrap_or_default();
    if SECRET_FINAL_WORDS.contains(&last_word) {
        return EnvClass::Secret;
    }
    EnvClass::Identity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_renders_its_value() {
        let secret = SecretValue::new("ghp_supersecret");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert_eq!(secret.expose(), "ghp_supersecret");
    }

    #[test]
    fn credential_shaped_keys_are_secret() {
        for key in [
            "GITHUB_TOKEN",
            "GITHUB_PERSONAL_ACCESS_TOKEN",
            "OPENAI_API_KEY",
            "client_secret",
            "SLACK_BOT_TOKEN",
        ] {
            assert_eq!(classify_key(key), EnvClass::Secret, "{key}");
        }
    }

    #[test]
    fn a_location_suffix_outranks_a_credential_marker() {
        // The path selects the account; it is not the credential, and two
        // servers pointing at different token files are different servers.
        for key in ["GITHUB_TOKEN_FILE", "AUTH_PATH", "SECRET_DIR"] {
            assert_eq!(classify_key(key), EnvClass::Identity, "{key}");
        }
    }

    #[test]
    fn enumeration_valued_auth_keys_are_not_secret() {
        for key in ["AUTH_MODE", "AUTH_TYPE", "AUTHORITY"] {
            assert_eq!(classify_key(key), EnvClass::Identity, "{key}");
        }
    }

    #[test]
    fn shell_state_is_ambient() {
        for key in ["PATH", "HOME", "TERM", "LC_ALL", "LANG"] {
            assert_eq!(classify_key(key), EnvClass::Ambient, "{key}");
        }
    }

    #[test]
    fn an_ordinary_selector_is_identity_bearing() {
        // This is the case that keeps `short-term-memory` and
        // `long-term-memory` distinct: same command, same args, different file.
        assert_eq!(classify_key("MEMORY_FILE_PATH"), EnvClass::Identity);
        assert_eq!(classify_key("NODE_OPTIONS"), EnvClass::Identity);
        assert_eq!(classify_key("MAX_TOKENS"), EnvClass::Identity);
    }

    #[test]
    fn only_identity_values_are_offered_for_hashing() {
        assert_eq!(
            EnvValue::classify("MEMORY_FILE_PATH", "/a").identity_value(),
            Some("/a")
        );
        assert_eq!(
            EnvValue::classify("GITHUB_TOKEN", "ghp_x").identity_value(),
            None
        );
        assert_eq!(
            EnvValue::classify("PATH", "/usr/bin").identity_value(),
            None
        );
    }
}

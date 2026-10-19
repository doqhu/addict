//! Addict core: GTK-free product types (word, context, dictionaries, providers).

pub mod dictionary;
pub mod input;
pub mod reduction;
pub mod reduction_es_ca;
pub mod sentence;
pub mod translation;

/// Application id of the product; the single source of product identity.
pub const APP_ID: &str = "dev.addict.Addict";

#[cfg(test)]
mod tests {
    use super::APP_ID;

    #[test]
    fn app_id_value() {
        assert_eq!(APP_ID, "dev.addict.Addict");
    }

    #[test]
    fn app_id_is_reverse_dns() {
        let parts: Vec<&str> = APP_ID.split('.').collect();
        assert!(
            parts.len() >= 3,
            "reverse-DNS id needs at least three parts"
        );
        for part in parts {
            assert!(!part.is_empty());
            assert!(
                part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "invalid character in {part:?}"
            );
            assert!(!part.starts_with(|c: char| c.is_ascii_digit()));
        }
    }
}

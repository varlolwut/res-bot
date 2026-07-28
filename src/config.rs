use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub window_title_fragments: Vec<String>,
    pub poll_interval_seconds: u64,
    pub click_min_duration_ms: u64,
    pub click_max_duration_ms: u64,
    pub pre_click_min_delay_ms: u64,
    pub pre_click_max_delay_ms: u64,
}

impl Config {
    pub fn built_in() -> Self {
        Self {
            window_title_fragments: vec!["lineage".to_owned()],
            poll_interval_seconds: 10,
            click_min_duration_ms: 280,
            click_max_duration_ms: 650,
            pre_click_min_delay_ms: 180,
            pre_click_max_delay_ms: 480,
        }
    }

    pub fn load(executable_path: &Path) -> AppResult<Self> {
        let path = configuration_path(executable_path);
        if !path.exists() {
            return Self::built_in().validate();
        }

        let source = fs::read_to_string(&path).map_err(|source| AppError::ConfigRead {
            path: path.display().to_string(),
            source,
        })?;
        let config = toml::from_str::<Self>(&source).map_err(|source| AppError::ConfigParse {
            path: path.display().to_string(),
            source,
        })?;
        config.validate()
    }

    fn validate(self) -> AppResult<Self> {
        if self.window_title_fragments.is_empty()
            || self
                .window_title_fragments
                .iter()
                .any(|fragment| fragment.trim().is_empty())
        {
            return Err(AppError::ConfigValue {
                field: "window_title_fragments",
                value: format!("{:?}", self.window_title_fragments),
                reason: "must contain at least one non-empty fragment",
            });
        }
        if self.poll_interval_seconds == 0 {
            return Err(AppError::ConfigValue {
                field: "poll_interval_seconds",
                value: self.poll_interval_seconds.to_string(),
                reason: "must be greater than zero",
            });
        }
        validate_range(
            "click_duration_ms",
            self.click_min_duration_ms,
            self.click_max_duration_ms,
        )?;
        validate_range(
            "pre_click_delay_ms",
            self.pre_click_min_delay_ms,
            self.pre_click_max_delay_ms,
        )?;
        Ok(self)
    }
}

fn configuration_path(executable_path: &Path) -> PathBuf {
    executable_path.with_extension("toml")
}

fn validate_range(field: &'static str, minimum: u64, maximum: u64) -> AppResult<()> {
    if minimum == 0 || minimum > maximum {
        return Err(AppError::ConfigValue {
            field,
            value: format!("{minimum}..={maximum}"),
            reason: "minimum must be greater than zero and no greater than maximum",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn built_in_configuration_is_valid() {
        assert!(Config::built_in().validate().is_ok());
    }

    #[test]
    fn invalid_click_range_is_rejected() {
        let mut config = Config::built_in();
        config.click_min_duration_ms = 700;
        config.click_max_duration_ms = 300;

        assert!(config.validate().is_err());
    }
}

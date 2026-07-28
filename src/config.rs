use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetMode {
    Lineage,
    Parsec,
}

impl TargetMode {
    pub fn title_fragment(self) -> &'static str {
        match self {
            Self::Lineage => "lineage",
            Self::Parsec => "parsec",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    pub window_title_fragments: Vec<String>,
    pub poll_interval_seconds: u64,
    pub click_min_duration_ms: u64,
    pub click_max_duration_ms: u64,
    pub pre_click_min_delay_ms: u64,
    pub pre_click_max_delay_ms: u64,
    pub confirmation_frame_count: u32,
    pub confirmation_interval_ms: u64,
    pub mouse_idle_required_ms: u64,
    pub mouse_idle_timeout_ms: u64,
    pub relocate_cursor_after_click: bool,
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
            confirmation_frame_count: 2,
            confirmation_interval_ms: 150,
            mouse_idle_required_ms: 600,
            mouse_idle_timeout_ms: 3_000,
            relocate_cursor_after_click: true,
        }
    }

    pub fn load(executable_path: &Path) -> AppResult<Self> {
        let path = Self::path_for_executable(executable_path);
        if !path.exists() {
            let config = Self::built_in();
            config.validate()?;
            return Ok(config);
        }

        let source = fs::read_to_string(&path).map_err(|source| AppError::ConfigRead {
            path: path.display().to_string(),
            source,
        })?;
        let config = toml::from_str::<Self>(&source).map_err(|source| AppError::ConfigParse {
            path: path.display().to_string(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn path_for_executable(executable_path: &Path) -> PathBuf {
        executable_path.with_extension("toml")
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        self.validate()?;
        let source = toml::to_string_pretty(self).map_err(|source| AppError::ConfigSerialize {
            path: path.display().to_string(),
            source,
        })?;
        fs::write(path, source).map_err(|source| AppError::ConfigWrite {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn target_mode(&self) -> Option<TargetMode> {
        if self.window_title_fragments.len() != 1 {
            return None;
        }
        let fragment = self.window_title_fragments[0].trim();
        [TargetMode::Lineage, TargetMode::Parsec]
            .into_iter()
            .find(|mode| fragment.eq_ignore_ascii_case(mode.title_fragment()))
    }

    pub fn for_target_mode(&self, mode: TargetMode) -> Self {
        let mut updated = self.clone();
        updated.window_title_fragments = vec![mode.title_fragment().to_owned()];
        updated
    }

    pub fn validate(&self) -> AppResult<()> {
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
        if !(2..=5).contains(&self.confirmation_frame_count) {
            return Err(AppError::ConfigValue {
                field: "confirmation_frame_count",
                value: self.confirmation_frame_count.to_string(),
                reason: "must be between 2 and 5",
            });
        }
        validate_bounded(
            "confirmation_interval_ms",
            self.confirmation_interval_ms,
            50,
            1_000,
        )?;
        validate_bounded(
            "mouse_idle_required_ms",
            self.mouse_idle_required_ms,
            100,
            3_000,
        )?;
        validate_bounded(
            "mouse_idle_timeout_ms",
            self.mouse_idle_timeout_ms,
            self.mouse_idle_required_ms,
            10_000,
        )
    }
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

fn validate_bounded(field: &'static str, value: u64, minimum: u64, maximum: u64) -> AppResult<()> {
    if !(minimum..=maximum).contains(&value) {
        return Err(AppError::ConfigValue {
            field,
            value: value.to_string(),
            reason: "is outside the supported range",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn confirmation_requires_multiple_frames() {
        let mut config = Config::built_in();
        config.confirmation_frame_count = 1;

        assert!(config.validate().is_err());
    }

    #[test]
    fn built_in_configuration_round_trips_through_toml() {
        let config = Config::built_in();
        let source = toml::to_string(&config).unwrap();
        let decoded = toml::from_str::<Config>(&source).unwrap();

        assert_eq!(decoded, config);
    }

    #[test]
    fn saved_configuration_can_be_loaded_after_restart() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let executable = env::temp_dir().join(format!("res-bot-config-{unique}.exe"));
        let path = Config::path_for_executable(&executable);
        let config = Config::built_in();

        config.save(&path).unwrap();
        let loaded = Config::load(&executable).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(loaded, config);
    }

    #[test]
    fn target_mode_replaces_only_window_title_fragments() {
        let config = Config::built_in();
        let parsec = config.for_target_mode(super::TargetMode::Parsec);

        assert_eq!(parsec.target_mode(), Some(super::TargetMode::Parsec));
        assert_eq!(parsec.poll_interval_seconds, config.poll_interval_seconds);
        assert_eq!(
            parsec.relocate_cursor_after_click,
            config.relocate_cursor_after_click
        );
    }
}


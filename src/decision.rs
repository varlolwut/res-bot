use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResurrectionPercentage {
    Zero,
    Hundred,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Accept,
    Reject,
}

pub fn parse_percentage(text: &str) -> AppResult<Option<ResurrectionPercentage>> {
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let has_hundred = compact.contains("100%");
    let has_zero = contains_standalone_zero_percent(&compact);

    match (has_zero, has_hundred) {
        (true, true) => Err(AppError::ConflictingPercentage {
            text: text.to_owned(),
        }),
        (true, false) => Ok(Some(ResurrectionPercentage::Zero)),
        (false, true) => Ok(Some(ResurrectionPercentage::Hundred)),
        (false, false) => Ok(None),
    }
}

fn contains_standalone_zero_percent(text: &str) -> bool {
    text.match_indices("0%").any(|(index, _)| {
        text[..index]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_digit())
    })
}

pub fn has_numbered_nickname(text: &str) -> bool {
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>();

    let characters = compact.chars().collect::<Vec<char>>();
    characters.windows(4).any(|window| {
        window[0] == '_'
            && window[1].is_ascii_digit()
            && window[2].is_ascii_digit()
            && !window[3].is_ascii_digit()
    }) || characters
        .get(characters.len().saturating_sub(3)..)
        .is_some_and(|suffix| {
            suffix[0] == '_' && suffix[1].is_ascii_digit() && suffix[2].is_ascii_digit()
        })
}

pub fn choose_action(percentage: ResurrectionPercentage, numbered_nickname: bool) -> Action {
    match (numbered_nickname, percentage) {
        (true, _) | (false, ResurrectionPercentage::Hundred) => Action::Accept,
        (false, ResurrectionPercentage::Zero) => Action::Reject,
    }
}

#[cfg(test)]
mod tests {
    use crate::error::AppError;

    use super::{
        Action, ResurrectionPercentage, choose_action, has_numbered_nickname, parse_percentage,
    };

    #[test]
    fn parses_supported_percentages() {
        assert_eq!(
            parse_percentage("5,429,441,752 (100%). Вы согласны?").unwrap(),
            Some(ResurrectionPercentage::Hundred)
        );
        assert_eq!(
            parse_percentage("персонажа 0 (0%). Вы согласны?").unwrap(),
            Some(ResurrectionPercentage::Zero)
        );
    }

    #[test]
    fn conflicting_percentages_are_reported_explicitly() {
        let error = parse_percentage("0% and 100%").unwrap_err();

        assert!(matches!(error, AppError::ConflictingPercentage { .. }));
    }

    #[test]
    fn recognizes_exact_two_digit_suffix() {
        assert!(has_numbered_nickname("lonedy_42"));
        assert!(has_numbered_nickname(" OtherName _ 07 "));
        assert!(!has_numbered_nickname("lonedy"));
        assert!(!has_numbered_nickname("lonedy_7"));
        assert!(!has_numbered_nickname("lonedy_123"));
    }

    #[test]
    fn numbered_nickname_overrides_zero_percent() {
        assert_eq!(
            choose_action(ResurrectionPercentage::Zero, true),
            Action::Accept
        );
        assert_eq!(
            choose_action(ResurrectionPercentage::Zero, false),
            Action::Reject
        );
    }
}


pub(super) fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;

    for character in line.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            continue;
        }
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    word.push(character);
                }
            }
            Some('"') => match character {
                '"' => quote = None,
                '\\' => escaped = true,
                _ => word.push(character),
            },
            Some(_) => unreachable!(),
            None => match character {
                '#' => break,
                '\'' | '"' => quote = Some(character),
                '\\' => escaped = true,
                character if character.is_whitespace() => {
                    if !word.is_empty() {
                        words.push(std::mem::take(&mut word));
                    }
                }
                _ => word.push(character),
            },
        }
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

pub(super) fn wildcard_matches(pattern: &str, value: &str) -> bool {
    wildcard_matches_bytes(pattern.as_bytes(), value.as_bytes())
}

fn wildcard_matches_bytes(pattern: &[u8], value: &[u8]) -> bool {
    match pattern.split_first() {
        None => value.is_empty(),
        Some((&b'*', rest)) => {
            wildcard_matches_bytes(rest, value)
                || (!value.is_empty() && wildcard_matches_bytes(pattern, &value[1..]))
        }
        Some((&b'?', rest)) => !value.is_empty() && wildcard_matches_bytes(rest, &value[1..]),
        Some((&expected, rest)) => value.first().is_some_and(|actual| {
            expected.eq_ignore_ascii_case(actual) && wildcard_matches_bytes(rest, &value[1..])
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{wildcard_matches, words};

    #[test]
    fn lexes_quotes_escaped_spaces_and_comments() {
        assert_eq!(
            words(r#"IdentityFile "key path" escaped\ path # comment"#),
            vec!["IdentityFile", "key path", "escaped path"]
        );
        assert_eq!(words("Host one\\#two # comment"), vec!["Host", "one#two"]);
    }

    #[test]
    fn matches_openssh_style_wildcards() {
        assert!(wildcard_matches("*.corp", "one.CORP"));
        assert!(wildcard_matches("web-?", "web-a"));
        assert!(!wildcard_matches("web-?", "web-aa"));
    }
}

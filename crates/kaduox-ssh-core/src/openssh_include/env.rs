use anyhow::{Context, Result, bail};

pub(super) fn expand_include_environment(value: &str, max_bytes: usize) -> Result<String> {
    expand_include_environment_with(value, max_bytes, |name| {
        std::env::var(name).with_context(|| {
            format!("OpenSSH Include environment variable ${{{name}}} is not available as UTF-8")
        })
    })
}

fn expand_include_environment_with<F>(
    value: &str,
    max_bytes: usize,
    mut lookup: F,
) -> Result<String>
where
    F: FnMut(&str) -> Result<String>,
{
    let mut output = String::with_capacity(value.len().min(max_bytes));
    let mut rest = value;

    while let Some(start) = rest.find("${") {
        let literal = &rest[..start];
        reject_percent_tokens(literal)?;
        push_bounded(&mut output, literal, max_bytes)?;

        let variable_and_rest = &rest[start + 2..];
        let end = variable_and_rest
            .find('}')
            .context("OpenSSH Include contains an unterminated ${...} environment expansion")?;
        let name = &variable_and_rest[..end];
        if name.is_empty() {
            bail!("OpenSSH Include contains an empty environment variable name");
        }

        let value = lookup(name).with_context(|| {
            format!("failed to expand OpenSSH Include environment variable ${{{name}}}")
        })?;
        // Match OpenSSH's single-pass percent+dollar expansion: bytes produced
        // by an environment lookup are appended verbatim and are not rescanned
        // as percent tokens or nested ${...} expressions.
        push_bounded(&mut output, &value, max_bytes)?;
        rest = &variable_and_rest[end + 1..];
    }

    reject_percent_tokens(rest)?;
    push_bounded(&mut output, rest, max_bytes)?;
    Ok(output)
}

fn reject_percent_tokens(value: &str) -> Result<()> {
    if value.contains('%') {
        bail!("OpenSSH Include percent-token expansion is not supported yet");
    }
    Ok(())
}

fn push_bounded(output: &mut String, value: &str, max_bytes: usize) -> Result<()> {
    let new_len = output
        .len()
        .checked_add(value.len())
        .context("OpenSSH Include environment expansion byte accounting overflow")?;
    if new_len > max_bytes {
        bail!("expanded OpenSSH Include path exceeds the {max_bytes}-byte safety limit");
    }
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Result<String> {
        match name {
            "CONF_ROOT" => Ok("conf.d".to_owned()),
            "FILE" => Ok("prod.conf".to_owned()),
            "SPACED" => Ok("dir with spaces".to_owned()),
            "PERCENT" => Ok("literal%h.conf".to_owned()),
            "NESTED" => Ok("${FILE}".to_owned()),
            "A%B" => Ok("percent-name.conf".to_owned()),
            _ => bail!("missing test variable {name}"),
        }
    }

    #[test]
    fn expands_multiple_environment_references_without_rescanning_values() {
        assert_eq!(
            expand_include_environment_with("${CONF_ROOT}/${FILE}", 1024, lookup).unwrap(),
            "conf.d/prod.conf"
        );
        assert_eq!(
            expand_include_environment_with("${NESTED}", 1024, lookup).unwrap(),
            "${FILE}"
        );
    }

    #[test]
    fn preserves_plain_dollar_and_environment_value_characters() {
        assert_eq!(
            expand_include_environment_with("price$5/${SPACED}/${PERCENT}", 1024, lookup)
                .unwrap(),
            "price$5/dir with spaces/literal%h.conf"
        );
        assert_eq!(
            expand_include_environment_with("${A%B}", 1024, lookup).unwrap(),
            "percent-name.conf"
        );
    }

    #[test]
    fn raw_percent_tokens_remain_fail_closed() {
        for input in ["%h.conf", "${CONF_ROOT}/%n.conf"] {
            assert!(
                expand_include_environment_with(input, 1024, lookup).is_err(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn malformed_or_missing_variables_fail_closed() {
        for input in ["${", "${}", "before-${MISSING}-after"] {
            assert!(
                expand_include_environment_with(input, 1024, lookup).is_err(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn expanded_size_is_bounded() {
        assert!(expand_include_environment_with("${SPACED}", 3, lookup).is_err());
        assert_eq!(
            expand_include_environment_with("abc", 3, lookup).unwrap(),
            "abc"
        );
    }
}

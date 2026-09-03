use std::collections::BTreeSet;

use anyhow::{Result, bail};

use crate::client::quote_posix;

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteCommandSpec {
    pub program: String,
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub working_directory: Option<String>,
}

impl RemoteCommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            environment: Vec::new(),
            working_directory: None,
        }
    }

    pub fn validated(&self) -> Result<()> {
        if self.program.is_empty() {
            bail!("remote command program cannot be empty");
        }
        reject_nul("remote command program", &self.program)?;
        for argument in &self.arguments {
            reject_nul("remote command argument", argument)?;
        }

        if let Some(directory) = &self.working_directory {
            if directory.is_empty() {
                bail!("remote working directory cannot be empty");
            }
            reject_nul("remote working directory", directory)?;
        }

        let mut names = BTreeSet::new();
        for (name, value) in &self.environment {
            if !valid_environment_name(name) {
                bail!("invalid remote environment variable name: {name}");
            }
            if !names.insert(name.as_str()) {
                bail!("duplicate remote environment variable: {name}");
            }
            reject_nul("remote environment value", value)?;
        }
        Ok(())
    }

    pub fn render_posix(&self) -> Result<String> {
        self.validated()?;

        let mut rendered = String::new();
        if let Some(directory) = &self.working_directory {
            let directory = safe_cd_operand(directory);
            rendered.push_str("cd ");
            rendered.push_str(&quote_posix(&directory));
            rendered.push_str(" && ");
        }

        for (name, value) in &self.environment {
            rendered.push_str(name);
            rendered.push('=');
            rendered.push_str(&quote_posix(value));
            rendered.push(' ');
        }

        rendered.push_str(&quote_posix(&self.program));
        for argument in &self.arguments {
            rendered.push(' ');
            rendered.push_str(&quote_posix(argument));
        }
        Ok(rendered)
    }
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn reject_nul(label: &str, value: &str) -> Result<()> {
    if value.contains('\0') {
        bail!("{label} cannot contain NUL bytes");
    }
    Ok(())
}

fn safe_cd_operand(directory: &str) -> String {
    if directory.starts_with('-') {
        format!("./{directory}")
    } else {
        directory.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_arguments_environment_and_working_directory() {
        let spec = RemoteCommandSpec {
            program: "printf".to_owned(),
            arguments: vec!["%s %s\\n".to_owned(), "hello world".to_owned(), "it's-safe".to_owned()],
            environment: vec![("APP_ENV".to_owned(), "prod west".to_owned())],
            working_directory: Some("/srv/app release".to_owned()),
        };

        assert_eq!(
            spec.render_posix().unwrap(),
            "cd '/srv/app release' && APP_ENV='prod west' printf '%s %s\\n' 'hello world' 'it'\"'\"'s-safe'"
        );
    }

    #[test]
    fn preserves_existing_simple_exec_rendering() {
        let spec = RemoteCommandSpec {
            program: "uname".to_owned(),
            arguments: vec!["-a".to_owned()],
            environment: Vec::new(),
            working_directory: None,
        };
        assert_eq!(spec.render_posix().unwrap(), "uname -a");
    }

    #[test]
    fn rejects_invalid_or_duplicate_environment_names() {
        for name in ["", "9PORT", "BAD-NAME", "A=B", "你好"] {
            let spec = RemoteCommandSpec {
                program: "true".to_owned(),
                arguments: Vec::new(),
                environment: vec![(name.to_owned(), "value".to_owned())],
                working_directory: None,
            };
            assert!(spec.validated().is_err(), "{name}");
        }

        let duplicate = RemoteCommandSpec {
            program: "true".to_owned(),
            arguments: Vec::new(),
            environment: vec![
                ("MODE".to_owned(), "one".to_owned()),
                ("MODE".to_owned(), "two".to_owned()),
            ],
            working_directory: None,
        };
        assert!(duplicate.validated().is_err());
    }

    #[test]
    fn protects_leading_dash_working_directory() {
        let spec = RemoteCommandSpec {
            program: "pwd".to_owned(),
            arguments: Vec::new(),
            environment: Vec::new(),
            working_directory: Some("-release".to_owned()),
        };
        assert_eq!(spec.render_posix().unwrap(), "cd ./-release && pwd");
    }

    #[test]
    fn rejects_nul_in_shell_inputs() {
        let mut spec = RemoteCommandSpec::new("printf");
        spec.arguments.push("bad\0arg".to_owned());
        assert!(spec.validated().is_err());
    }
}

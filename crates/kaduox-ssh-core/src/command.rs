use std::collections::BTreeSet;

use anyhow::{Result, bail};

use crate::client::{CommandOutput, RemoteUser, SshClient, quote_posix};

#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Windows cmd.exe 兼容渲染：空格连接、含空白/引号的参数用双引号包裹。
    /// POSIX 单引号渲染经 sshd 的 `cmd /c "<command>"` 包装后会被 cmd 的
    /// 引号剥离规则破坏（残留引号导致 "not recognized"），Windows 目标必须用它。
    /// 程序名 `cmd` 统一渲染为 `cmd.exe`（实测裸 `cmd` 会触发同样的引号怪癖）。
    pub fn render_cmd(&self) -> Result<String> {
        self.validated()?;

        let mut rendered = String::new();
        if let Some(directory) = &self.working_directory {
            rendered.push_str("cd /d ");
            rendered.push_str(&quote_cmd(directory));
            rendered.push_str(" && ");
        }
        for (name, value) in &self.environment {
            rendered.push_str("set ");
            rendered.push_str(name);
            rendered.push('=');
            rendered.push_str(&quote_cmd(value));
            rendered.push_str(" && ");
        }

        let program = if self.program.eq_ignore_ascii_case("cmd") {
            "cmd.exe"
        } else {
            self.program.as_str()
        };
        rendered.push_str(&quote_cmd(program));
        for argument in &self.arguments {
            rendered.push(' ');
            rendered.push_str(&quote_cmd(argument));
        }
        Ok(rendered)
    }
}

/// cmd 双引号转义：内部 `"` 翻倍。含空白/引号/ cmd 元字符时才加引号。
fn quote_cmd(token: &str) -> String {
    if !token.is_empty()
        && !token
            .contains(|c: char| c.is_whitespace() || matches!(c, '"' | '&' | '|' | '<' | '>' | '^'))
    {
        return token.to_owned();
    }
    format!("\"{}\"", token.replace('"', "\"\""))
}

impl SshClient {
    pub async fn exec_spec(
        &self,
        command: &RemoteCommandSpec,
        remote_user: &RemoteUser,
    ) -> Result<CommandOutput> {
        self.exec(&command.render_posix()?, remote_user).await
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
            arguments: vec![
                "%s %s\\n".to_owned(),
                "hello world".to_owned(),
                "it's-safe".to_owned(),
            ],
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

    #[test]
    fn render_cmd_avoids_posix_quotes_and_normalizes_cmd() {
        let spec = RemoteCommandSpec {
            program: "cmd".to_owned(),
            arguments: vec!["/c".to_owned(), "ver".to_owned()],
            environment: Vec::new(),
            working_directory: None,
        };
        assert_eq!(spec.render_cmd().unwrap(), "cmd.exe /c ver");

        let spec = RemoteCommandSpec {
            program: "echo".to_owned(),
            arguments: vec!["hello world".to_owned()],
            environment: Vec::new(),
            working_directory: None,
        };
        assert_eq!(spec.render_cmd().unwrap(), "echo \"hello world\"");

        let spec = RemoteCommandSpec {
            program: "echo".to_owned(),
            arguments: vec!["say \"hi\"".to_owned()],
            environment: Vec::new(),
            working_directory: None,
        };
        assert_eq!(spec.render_cmd().unwrap(), "echo \"say \"\"hi\"\"\"");
    }
}

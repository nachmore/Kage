//! The credential guard: commands that look like they touch credentials
//! always ask the user, whatever rules or switches say.
//!
//! The agent's credential sandbox exists to stop credentials being used by
//! accident; running a command outside it must not quietly undo that. So a
//! command that names an AWS / SSH / cloud credential store or the macOS
//! keychain, asks a CLI for credentials, or dumps the environment is
//! never run without the user's own click, and the prompt says why.
//!
//! Best-effort by design: it reads the command's text. A script, an
//! alias, a program that reads the files itself or an obfuscated path
//! gets past it, which is why rules only ever cover simple commands and
//! the user still reads what runs. A false alarm costs a click.

use serde::Serialize;

/// What the guard found. Wire names are the prompt's warning ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialHit {
    /// `~/.aws`, `AWS_SHARED_CREDENTIALS_FILE`, `aws configure
    /// export-credentials`, `aws sts get-session-token`, …
    Aws,
    /// A credential sandbox's session file (`.sbox-session`).
    SandboxSession,
    /// `~/.ssh`, private key files (`id_rsa`, `id_ed25519`, …).
    Ssh,
    /// `.netrc` / `_netrc`.
    Netrc,
    /// `gh`'s `hosts.yml`, `gh auth token`, `.git-credentials`, `git
    /// credential`.
    Git,
    /// gcloud credentials and access tokens.
    Gcloud,
    /// Azure, Kubernetes and Docker credential files.
    Cloud,
    /// The macOS keychain (`security find-*-password`, `dump-keychain`, …).
    Keychain,
    /// A dump of the whole environment (`env`, `printenv`, `set`, …).
    EnvDump,
    /// A credential environment variable (`AWS_SECRET_ACCESS_KEY`, …).
    CredentialEnv,
    /// A glob into a hidden folder (`~/.a*/cred*`), which could resolve to
    /// any of the above.
    HiddenGlob,
}

/// Characters that end a path component (or a word) on either side.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// Whether `text` holds `name` as a whole path component: `~/.aws`,
/// `$HOME/.aws/config`, `cd .aws` — not `foo.aws` or `.awsrc`.
fn has_component(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + name.len()..].chars().next();
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

/// The command as the guard reads it: lower-cased (macOS paths are
/// case-insensitive), quotes removed (`~/.a""ws`), `${HOME}` and `$HOME`
/// spelled the same, and each backslash replaced by `backslash` — dropped
/// (`~/.a\ws` is `~/.aws` to a POSIX shell) or read as a Windows path
/// separator (`%USERPROFILE%\.aws`).
fn normalize(command: &str, backslash: Option<char>) -> String {
    command
        .chars()
        .filter(|c| !matches!(c, '\'' | '"'))
        .filter_map(|c| if c == '\\' { backslash } else { Some(c) })
        .collect::<String>()
        .to_lowercase()
        .replace("${home}", "$home")
}

/// Credential files and folders named in `text`.
fn file_hits(text: &str, add: &mut impl FnMut(CredentialHit)) {
    if has_component(text, ".aws")
        || text.contains("aws_shared_credentials_file")
        || text.contains("aws_config_file")
    {
        add(CredentialHit::Aws);
    }
    if text.contains(".sbox-session") {
        add(CredentialHit::SandboxSession);
    }
    if has_component(text, ".ssh")
        || ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"]
            .iter()
            .any(|k| text.contains(k))
    {
        add(CredentialHit::Ssh);
    }
    if text.contains(".netrc") || text.contains("_netrc") {
        add(CredentialHit::Netrc);
    }
    if text.contains("hosts.yml") || text.contains(".git-credentials") {
        add(CredentialHit::Git);
    }
    if text.contains(".config/gcloud")
        || text.contains("application_default_credentials")
        || text.contains("credentials.db")
        || text.contains("access_tokens.db")
    {
        add(CredentialHit::Gcloud);
    }
    if has_component(text, ".azure")
        || text.contains(".kube/config")
        || text.contains(".docker/config.json")
    {
        add(CredentialHit::Cloud);
    }
    if text.contains("library/keychains") || text.contains(".keychain") {
        add(CredentialHit::Keychain);
    }
    if text.contains("/environ") || text.contains("getenvironmentvariables") {
        add(CredentialHit::EnvDump);
    }
    if CREDENTIAL_ENV_VARS.iter().any(|v| text.contains(v)) {
        add(CredentialHit::CredentialEnv);
    }
}

/// Simple commands of `text`, split on separators, as words.
fn segments(text: &str) -> Vec<Vec<&str>> {
    text.split([';', '&', '|', '\n', '(', ')', '`', '{', '}'])
        .map(|seg| {
            seg.split([' ', '\t', '\r'])
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|ws| !ws.is_empty())
        .collect()
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The words after the program (assignments, `sudo`, `env VAR=…`,
/// `command`, `exec`, `time`, `nohup` skipped), and the program.
fn program_and_args<'a>(words: &[&'a str]) -> Option<(&'a str, Vec<&'a str>)> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let is_assignment = w.split_once('=').is_some_and(|(n, _)| {
            !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if is_assignment
            || matches!(
                w,
                "sudo" | "doas" | "command" | "exec" | "time" | "nohup" | "$(" | "$"
            )
        {
            i += 1;
            continue;
        }
        break;
    }
    let program = basename(words.get(i)?);
    Some((program, words[i + 1..].to_vec()))
}

/// Whether `args` holds `seq` in order (other words, like options, may sit
/// between).
fn has_in_order(args: &[&str], seq: &[&str]) -> bool {
    let mut it = args.iter();
    seq.iter().all(|want| {
        it.any(|a| a == want || (want.ends_with('*') && a.starts_with(&want[..want.len() - 1])))
    })
}

const CREDENTIAL_ENV_VARS: &[&str] = &[
    "aws_secret_access_key",
    "aws_session_token",
    "aws_access_key_id",
    "aws_security_token",
    "github_token",
    "gh_token",
    "google_application_credentials",
    "azure_client_secret",
];

/// Credential stores a working directory lies in (`~/.aws`, `~/.ssh`, …):
/// there, `cat credentials` or `tar czf - .` reads them without naming
/// them. The path as given and, when it resolves, as the folder it really
/// is (a symlink in the agent's workspace can point into `~/.aws`).
pub fn path_hits(path: &std::path::Path) -> Vec<CredentialHit> {
    let mut hits: Vec<CredentialHit> = Vec::new();
    let mut add = |h: CredentialHit| {
        if !hits.contains(&h) {
            hits.push(h);
        }
    };
    let real = std::fs::canonicalize(path).ok();
    for p in std::iter::once(path).chain(real.as_deref()) {
        let text = p.to_string_lossy();
        file_hits(&normalize(&text, None), &mut add);
        file_hits(&normalize(&text, Some('/')), &mut add);
    }
    hits
}

/// Everything the guard flags in a request: its command
/// ([`credential_hits`]) and the folder it runs in ([`path_hits`]), each
/// once, in a stable order.
pub fn request_hits(command: &str, cwd: &std::path::Path) -> Vec<CredentialHit> {
    let mut hits = credential_hits(command);
    for h in path_hits(cwd) {
        if !hits.contains(&h) {
            hits.push(h);
        }
    }
    hits
}

/// Everything the guard flags in `command`, each once, in a stable order.
pub fn credential_hits(command: &str) -> Vec<CredentialHit> {
    let text = normalize(command, None);
    let mut hits: Vec<CredentialHit> = Vec::new();
    let mut add = |h: CredentialHit| {
        if !hits.contains(&h) {
            hits.push(h);
        }
    };

    // Files and folders, with a backslash read both ways.
    file_hits(&text, &mut add);
    file_hits(&normalize(command, Some('/')), &mut add);

    // Programs asked for credentials, and environment dumps.
    for words in segments(&text) {
        if words.iter().any(|w| {
            let w = w.trim_start_matches("./");
            (w.contains(['*', '?', '[']))
                && (w.starts_with('.') || w.contains("/.") || w.starts_with("~."))
        }) {
            add(CredentialHit::HiddenGlob);
        }
        // PowerShell's environment drive: `ls env:`, `gci Env:\`, `$env:X`.
        if words
            .iter()
            .any(|w| w.trim_start_matches('$').starts_with("env:"))
        {
            add(CredentialHit::EnvDump);
        }
        let Some((program, args)) = program_and_args(&words) else {
            continue;
        };
        let found = match program {
            "aws" => {
                let creds = [
                    &["configure", "export-credentials"][..],
                    &["configure", "get"],
                    &["sts", "get-session-token"],
                    &["sts", "assume-role*"],
                    &["sts", "get-federation-token"],
                    &["sso", "get-role-credentials"],
                    &["ecr", "get-login-password"],
                    &["codeartifact", "get-authorization-token"],
                    &["iam", "create-access-key"],
                    &["secretsmanager", "get-secret-value"],
                    &["secretsmanager", "batch-get-secret-value"],
                ];
                creds
                    .iter()
                    .any(|seq| has_in_order(&args, seq))
                    .then_some(CredentialHit::Aws)
            }
            "security" => args
                .iter()
                .any(|a| {
                    (a.starts_with("find-")
                        && (a.ends_with("-password") || a == &"find-certificate"))
                        || matches!(*a, "dump-keychain" | "export" | "export-keychain")
                })
                .then_some(CredentialHit::Keychain),
            "gh" => (has_in_order(&args, &["auth", "token"])
                || has_in_order(&args, &["auth", "status", "--show-token"])
                || has_in_order(&args, &["auth", "status", "-t"]))
            .then_some(CredentialHit::Git),
            "git" => (has_in_order(&args, &["credential"])
                || args.first() == Some(&"credential-osxkeychain"))
            .then_some(CredentialHit::Git),
            "gcloud" => args
                .iter()
                .any(|a| {
                    matches!(
                        *a,
                        "print-access-token" | "print-identity-token" | "print-refresh-token"
                    )
                })
                .then_some(CredentialHit::Gcloud),
            // Only options: the whole environment.
            "env" | "printenv" => args
                .iter()
                .all(|a| a.starts_with('-'))
                .then_some(CredentialHit::EnvDump),
            // `ps e`, `ps -E`, `ps eww`: every process's environment (BSD
            // and macOS; `-e` there too). Linux's `ps -ef` is a false alarm.
            "ps" => args
                .iter()
                .enumerate()
                .any(|(i, a)| {
                    let short = a.starts_with('-') && !a.starts_with("--");
                    let bsd = i == 0 && a.chars().all(|c| c.is_ascii_alphabetic());
                    (short || bsd) && a.contains('e')
                })
                .then_some(CredentialHit::EnvDump),
            // `set -e` sets an option; `set` alone lists variables.
            "set" => args.is_empty().then_some(CredentialHit::EnvDump),
            "export" => (args.is_empty() || args == ["-p"]).then_some(CredentialHit::EnvDump),
            "compgen" => {
                (args.contains(&"-v") || args.contains(&"-e")).then_some(CredentialHit::EnvDump)
            }
            // `declare -p`, `typeset -x`: listings; `declare -f`: functions.
            "declare" | "typeset" => (args.iter().all(|a| a.starts_with('-'))
                && (args.is_empty() || args.iter().any(|a| a.contains('p') || a.contains('x'))))
            .then_some(CredentialHit::EnvDump),
            _ => None,
        };
        if let Some(hit) = found {
            add(hit);
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::CredentialHit::*;
    use super::*;

    fn hits(cmd: &str) -> Vec<CredentialHit> {
        credential_hits(cmd)
    }

    #[test]
    fn aws_credential_stores_and_commands() {
        for cmd in [
            "cat ~/.aws/credentials",
            "cat $HOME/.aws/config",
            "cat ${HOME}/.aws/config",
            "cp -r /Users/me/.aws /tmp/x",
            "cd ~ && cat .aws/credentials",
            "less ~/.AWS/credentials",
            "cat ~/.a\"\"ws/credentials",
            "cat ~/.a\\ws/credentials",
            "AWS_SHARED_CREDENTIALS_FILE=/tmp/c aws s3 ls",
            "aws configure export-credentials --profile dev",
            "aws --profile dev configure get aws_secret_access_key",
            "aws sts get-session-token",
            "aws sts assume-role --role-arn x --role-session-name y",
            "aws sso get-role-credentials --role-name x",
            "aws ecr get-login-password | docker login --password-stdin x",
            "/usr/local/bin/aws configure export-credentials",
            r"type %USERPROFILE%\.aws\credentials",
        ] {
            assert!(hits(cmd).contains(&Aws), "{cmd}: {:?}", hits(cmd));
        }
    }

    #[test]
    fn ssh_netrc_git_gcloud_cloud() {
        assert!(hits("cat ~/.ssh/id_ed25519").contains(&Ssh));
        assert!(hits("scp id_rsa host:").contains(&Ssh));
        assert!(hits("ls -la ~/.ssh").contains(&Ssh));
        assert!(hits("cat ~/.netrc").contains(&Netrc));
        assert!(hits("cat %USERPROFILE%/_netrc").contains(&Netrc));
        assert!(hits("cat ~/.config/gh/hosts.yml").contains(&Git));
        assert!(hits("gh auth token").contains(&Git));
        assert!(hits("cat ~/.git-credentials").contains(&Git));
        assert!(hits("printf 'host=github.com\\n' | git credential fill").contains(&Git));
        assert!(
            hits("cat ~/.config/gcloud/application_default_credentials.json").contains(&Gcloud)
        );
        assert!(hits("gcloud auth print-access-token").contains(&Gcloud));
        assert!(hits("gcloud auth application-default print-access-token").contains(&Gcloud));
        assert!(hits("cat ~/.azure/accessTokens.json").contains(&Cloud));
        assert!(hits("cat ~/.kube/config").contains(&Cloud));
        assert!(hits("cat ~/.docker/config.json").contains(&Cloud));
        assert!(hits("cat ~/.sbox-session").contains(&SandboxSession));
    }

    #[test]
    fn keychain() {
        for cmd in [
            "security find-generic-password -s foo -w",
            "security find-internet-password -s github.com",
            "security dump-keychain",
            "security export -k login.keychain",
            "sudo security find-generic-password -wa x",
            "cp ~/Library/Keychains/login.keychain-db /tmp",
        ] {
            assert!(hits(cmd).contains(&Keychain), "{cmd}");
        }
        assert!(!hits("security list-keychains").contains(&Keychain));
    }

    #[test]
    fn environment_dumps_but_not_env_running_a_program() {
        for cmd in [
            "env",
            "env -0",
            "printenv",
            " set",
            "export",
            "export -p",
            "declare -p",
            "typeset",
            "compgen -v",
            "cat /proc/self/environ",
            "ls; env",
        ] {
            assert!(hits(cmd).contains(&EnvDump), "{cmd}");
        }
        for cmd in [
            "env FOO=1 make",
            "env -i make",
            "printenv HOME",
            "export PATH=/x:$PATH",
            "set -e",
        ] {
            assert!(!hits(cmd).contains(&EnvDump), "{cmd}");
        }
        assert!(hits("printenv AWS_SECRET_ACCESS_KEY").contains(&CredentialEnv));
        assert!(hits("echo $AWS_SESSION_TOKEN").contains(&CredentialEnv));
    }

    #[test]
    fn globs_into_hidden_folders() {
        assert!(hits("cat ~/.a?s/cred*").contains(&HiddenGlob));
        assert!(hits("tar czf x.tgz ~/.*").contains(&HiddenGlob));
        assert!(!hits("ls ~/Downloads/*.dmg").contains(&HiddenGlob));
        assert!(!hits("rm ./build/*.o").contains(&HiddenGlob));
    }

    #[test]
    fn everyday_commands_are_left_alone() {
        for cmd in [
            "brew install awscli",
            "brew install --cask docker",
            "npm install -g aws-cdk",
            "pip install awscli",
            "aws s3 ls",
            "aws --version",
            "git push origin main",
            "gh pr list",
            "ssh-keygen -t ed25519 -f /tmp/test_key",
            "gcloud config list",
            "make install",
            "echo foo.aws",
            "ls ~/.awsome-project",
        ] {
            assert_eq!(hits(cmd), Vec::<CredentialHit>::new(), "{cmd}");
        }
    }

    #[test]
    fn more_secret_reads() {
        for (cmd, hit) in [
            ("aws secretsmanager get-secret-value --secret-id x", Aws),
            ("aws iam create-access-key --user-name me", Aws),
            ("gh auth status -t", Git),
            ("ls env:", EnvDump),
            ("Get-ChildItem Env:\\", EnvDump),
            ("gci env:AWS_PROFILE", EnvDump),
            ("echo $env:PATH", EnvDump),
            ("[Environment]::GetEnvironmentVariables()", EnvDump),
            ("ps eww", EnvDump),
            ("ps -E -p 1", EnvDump),
        ] {
            assert!(hits(cmd).contains(&hit), "{cmd}: {:?}", hits(cmd));
        }
        for cmd in [
            "ps aux",
            "ps -ax",
            "ps -o user,pid",
            "aws secretsmanager list-secrets",
        ] {
            assert_eq!(hits(cmd), Vec::<CredentialHit>::new(), "{cmd}");
        }
    }

    #[test]
    fn a_working_directory_inside_a_credential_store_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let aws = tmp.path().join(".aws");
        std::fs::create_dir(&aws).unwrap();
        // `cat credentials` names nothing; the folder it runs in does.
        assert_eq!(credential_hits("cat credentials"), vec![]);
        assert_eq!(request_hits("cat credentials", &aws), vec![Aws]);
        assert_eq!(path_hits(&tmp.path().join(".ssh")), vec![Ssh]);

        // A link elsewhere that leads there.
        #[cfg(unix)]
        {
            let link = tmp.path().join("project");
            std::os::unix::fs::symlink(&aws, &link).unwrap();
            assert_eq!(request_hits("cat credentials", &link), vec![Aws]);
        }

        // Command and folder findings are merged, each once.
        assert_eq!(request_hits("cat ~/.aws/config", &aws), vec![Aws]);
        assert_eq!(request_hits("ls", tmp.path()), vec![]);
    }

    #[test]
    fn each_hit_once() {
        let h = hits("cat ~/.aws/credentials ~/.aws/config; aws sts get-session-token");
        assert_eq!(h.iter().filter(|x| **x == Aws).count(), 1);
    }
}

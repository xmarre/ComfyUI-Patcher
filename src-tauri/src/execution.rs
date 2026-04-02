use regex::RegexBuilder;
use std::path::Path;
use std::process::{Output, Stdio};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tokio::process::{Child, Command};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslPath {
    pub distro: String,
    pub linux_path: String,
}

pub fn parse_wsl_unc_path(path: &Path) -> Option<WslPath> {
    let raw = path.to_string_lossy().replace('/', "\\");
    let re = RegexBuilder::new(
        r"^(?:\\\\\?\\UNC\\|\\\\)(?:wsl\.localhost|wsl\$)\\(?P<distro>[^\\]+)(?P<rest>\\.*)?$",
    )
    .case_insensitive(true)
    .build()
    .unwrap();
    let caps = re.captures(&raw)?;
    let distro = caps.name("distro")?.as_str().to_string();
    let rest = caps.name("rest").map(|m| m.as_str()).unwrap_or("");
    let linux_path = if rest.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", rest.trim_start_matches('\\').replace('\\', "/"))
    };
    Some(WslPath { distro, linux_path })
}

fn is_wsl_launcher(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("wsl.exe") || name.eq_ignore_ascii_case("wsl")
        })
}

fn build_command(program: &str, args: &[String], cwd: Option<&Path>) -> std::io::Result<Command> {
    let cwd_wsl = cwd.and_then(parse_wsl_unc_path);
    let program_wsl = parse_wsl_unc_path(Path::new(program));

    if is_wsl_launcher(program) {
        let mut command = Command::new(program);
        command.args(args);
        if cwd_wsl.is_none() {
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
        }
        return Ok(command);
    }

    if let Some(context) = cwd_wsl.as_ref().or(program_wsl.as_ref()) {
        if let (Some(cwd_wsl), Some(program_wsl)) = (cwd_wsl.as_ref(), program_wsl.as_ref()) {
            if cwd_wsl.distro != program_wsl.distro {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "WSL cwd distro '{}' does not match program distro '{}'",
                        cwd_wsl.distro, program_wsl.distro
                    ),
                ));
            }
        }
        let mut command = Command::new("wsl.exe");
        command.arg("-d").arg(&context.distro);
        if let Some(cwd_wsl) = cwd_wsl.as_ref() {
            command.arg("--cd").arg(&cwd_wsl.linux_path);
        }
        let effective_program = match program_wsl.as_ref() {
            Some(wsl_program) if wsl_program.distro == context.distro => {
                wsl_program.linux_path.as_str()
            }
            _ => program,
        };
        command.arg("--").arg(effective_program);
        command.args(args);
        return Ok(command);
    }

    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    Ok(command)
}

pub(crate) fn configure_managed_spawn_command(command: &mut Command) {
    command.kill_on_drop(true);
}

pub(crate) fn configure_hidden_output_command(command: &mut Command) {
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    }
}

pub async fn output_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> std::io::Result<Output> {
    let mut command = build_command(program, args, cwd)?;
    configure_hidden_output_command(&mut command);
    command.output().await
}

pub fn spawn_command(program: &str, args: &[String], cwd: Option<&Path>) -> std::io::Result<Child> {
    let mut command = build_command(program, args, cwd)?;
    configure_managed_spawn_command(&mut command);
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_extended_wsl_unc_paths() {
        let parsed = parse_wsl_unc_path(Path::new(
            r"\\?\UNC\wsl.localhost\Ubuntu-22.04\home\toor\ComfyUI\custom_nodes",
        ))
        .unwrap();
        assert_eq!(parsed.distro, "Ubuntu-22.04");
        assert_eq!(parsed.linux_path, "/home/toor/ComfyUI/custom_nodes");
    }

    #[test]
    fn parses_plain_wsl_unc_paths() {
        let parsed = parse_wsl_unc_path(Path::new(
            r"\\wsl.localhost\Ubuntu-22.04\home\toor\ComfyUI",
        ))
        .unwrap();
        assert_eq!(parsed.distro, "Ubuntu-22.04");
        assert_eq!(parsed.linux_path, "/home/toor/ComfyUI");
    }

    #[test]
    fn parses_uppercase_wsl_hostnames() {
        let parsed = parse_wsl_unc_path(Path::new(
            r"\\WSL.LOCALHOST\Ubuntu-22.04\home\toor\ComfyUI",
        ))
        .unwrap();
        assert_eq!(parsed.distro, "Ubuntu-22.04");
        assert_eq!(parsed.linux_path, "/home/toor/ComfyUI");
    }

    #[test]
    fn ignores_non_wsl_paths() {
        assert!(parse_wsl_unc_path(&PathBuf::from(r"C:\ComfyUI")).is_none());
        assert!(parse_wsl_unc_path(Path::new("/home/toor/ComfyUI")).is_none());
    }

    #[test]
    fn build_command_translates_wsl_unc_programs() {
        let cwd = Path::new(r"\\wsl.localhost\Ubuntu-22.04\home\toor\ComfyUI");
        let program = r"\\wsl.localhost\Ubuntu-22.04\home\toor\miniconda3\envs\comfy\bin\python";
        let command = build_command(program, &["-m".to_string(), "pip".to_string()], Some(cwd))
            .unwrap();
        let program_dbg = format!("{:?}", command.as_std().get_program());
        let args_dbg = format!("{:?}", command.as_std().get_args().collect::<Vec<_>>());
        assert!(program_dbg.contains("wsl.exe"));
        assert!(args_dbg.contains("/home/toor/miniconda3/envs/comfy/bin/python"));
        assert!(!args_dbg.contains(r"\\wsl.localhost\Ubuntu-22.04\home\toor\miniconda3\envs\comfy\bin\python"));
    }

    #[test]
    fn build_command_rejects_mixed_wsl_distros() {
        let cwd = Path::new(r"\\wsl.localhost\Ubuntu-22.04\home\toor\ComfyUI");
        let program = r"\\wsl.localhost\Debian\home\toor\bin\python";
        let err = build_command(program, &[], Some(cwd)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn build_command_does_not_rewrap_explicit_wsl_launcher_with_wsl_unc_cwd() {
        let cwd = Path::new(r"\\wsl.localhost\Ubuntu-22.04\home\toor\ComfyUI");
        let args = vec![
            "-d".to_string(),
            "Ubuntu-22.04".to_string(),
            "--".to_string(),
            "/home/toor/start_comfyui.sh".to_string(),
            "--front-end-root".to_string(),
            "/mnt/c/Users/marre/source/repos/ComfyUI_frontend/dist".to_string(),
        ];
        let command = build_command("wsl.exe", &args, Some(cwd)).unwrap();

        let program_dbg = format!("{:?}", command.as_std().get_program());
        let args_dbg = format!("{:?}", command.as_std().get_args().collect::<Vec<_>>());

        assert!(program_dbg.contains("wsl.exe"));
        assert!(args_dbg.contains("start_comfyui.sh"));
        assert!(!args_dbg.contains("--cd"));
    }
}

use crate::errors::{AppError, AppResult};
use crate::execution::{
    configure_hidden_output_command, configure_managed_spawn_command, output_command,
    parse_wsl_unc_path, spawn_command,
};
use crate::models::LaunchProfile;
use std::collections::HashMap;
use std::path::Path;
use std::process::Output;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

#[derive(Clone)]
pub struct ProcessRegistry {
    inner: Arc<Mutex<HashMap<String, Child>>>,
}

const STOP_HELPER_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_WAIT_TIMEOUT: Duration = Duration::from_secs(15);

impl ProcessRegistry {
    fn child_has_exited(child: &mut Child) -> AppResult<bool> {
        match child.try_wait() {
            Ok(Some(_status)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(AppError::Process(e.to_string())),
        }
    }

    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn is_explicit_wsl_launcher(program: &str) -> bool {
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("wsl.exe") || name.eq_ignore_ascii_case("wsl")
            })
    }

    fn command_is_wsl_backed(program: &str, cwd: Option<&str>) -> bool {
        Self::is_explicit_wsl_launcher(program)
            || parse_wsl_unc_path(Path::new(program)).is_some()
            || cwd
                .and_then(|value| parse_wsl_unc_path(Path::new(value)))
                .is_some()
    }

    fn rewrite_frontend_root_args_for_context(
        program: &str,
        args: &[String],
        cwd: Option<&str>,
    ) -> Vec<String> {
        let wsl_backed = Self::command_is_wsl_backed(program, cwd);
        let mut rewritten = Vec::with_capacity(args.len());
        let mut index = 0usize;

        while index < args.len() {
            let current = &args[index];
            if current == "--front-end-root" {
                rewritten.push(current.clone());
                if let Some(value) = args.get(index + 1) {
                    let converted = if wsl_backed {
                        parse_wsl_unc_path(Path::new(value))
                            .map(|parsed| parsed.linux_path)
                            .unwrap_or_else(|| value.clone())
                    } else {
                        value.clone()
                    };
                    rewritten.push(converted);
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }
            if let Some(value) = current.strip_prefix("--front-end-root=") {
                let converted = if wsl_backed {
                    parse_wsl_unc_path(Path::new(value))
                        .map(|parsed| parsed.linux_path)
                        .unwrap_or_else(|| value.to_string())
                } else {
                    value.to_string()
                };
                rewritten.push(format!("--front-end-root={converted}"));
                index += 1;
                continue;
            }
            rewritten.push(current.clone());
            index += 1;
        }
        rewritten
    }

    fn joined_args(base: &[String], extra: Option<&[String]>) -> Vec<String> {
        let mut joined = Vec::with_capacity(base.len() + extra.map_or(0, |values| values.len()));
        joined.extend(base.iter().cloned());
        if let Some(extra) = extra {
            joined.extend(extra.iter().cloned());
        }
        joined
    }

    fn validate_command_env_support(
        program: &str,
        cwd: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> AppResult<()> {
        if !env.is_some_and(|values| !values.is_empty()) {
            return Ok(());
        }
        let cwd = cwd.map(Path::new);
        let program_is_wsl = parse_wsl_unc_path(Path::new(program)).is_some();
        let cwd_is_wsl = cwd.and_then(parse_wsl_unc_path).is_some();
        if program_is_wsl || cwd_is_wsl {
            return Err(AppError::Process(
                "launch profiles with custom environment variables are not supported for WSL-backed installations"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn command_failed(program: &str, output: &Output) -> AppError {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("exit status {}", output.status)
        };
        AppError::Process(format!("command '{}' failed: {}", program, detail))
    }

    async fn reinsert_child(&self, installation_id: &str, child: Child) {
        let mut map = self.inner.lock().await;
        map.insert(installation_id.to_string(), child);
    }

    async fn wait_for_child_exit(
        &self,
        installation_id: &str,
        mut child: Child,
        context: &str,
    ) -> AppResult<bool> {
        match timeout(STOP_WAIT_TIMEOUT, child.wait()).await {
            Ok(Ok(_status)) => Ok(true),
            Ok(Err(err)) => {
                self.reinsert_child(installation_id, child).await;
                Err(AppError::Process(format!(
                    "failed waiting for process to exit after {}: {}",
                    context, err
                )))
            }
            Err(_) => {
                self.reinsert_child(installation_id, child).await;
                Err(AppError::Process(format!(
                    "timed out waiting for process to exit after {}",
                    context
                )))
            }
        }
    }

    async fn kill_and_wait_child(
        &self,
        installation_id: &str,
        mut child: Child,
        context: &str,
    ) -> AppResult<bool> {
        if let Err(err) = child.start_kill() {
            self.reinsert_child(installation_id, child).await;
            return Err(AppError::Process(format!(
                "failed to kill process after {}: {}",
                context, err
            )));
        }
        self.wait_for_child_exit(installation_id, child, context).await
    }

    fn spawn_profile_command(
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> AppResult<Child> {
        Self::validate_command_env_support(program, cwd, env)?;
        let cwd = cwd.map(Path::new);
        if env.is_some_and(|values| !values.is_empty()) {
            let mut command = Command::new(program);
            command.args(args);
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            if let Some(env) = env {
                command.envs(env);
            }
            configure_managed_spawn_command(&mut command);
            return command
                .spawn()
                .map_err(|e| AppError::Process(e.to_string()));
        }
        spawn_command(program, args, cwd).map_err(|e| AppError::Process(e.to_string()))
    }

    async fn run_profile_command(
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> AppResult<()> {
        Self::validate_command_env_support(program, cwd, env)?;
        let cwd = cwd.map(Path::new);
        let output = if env.is_some_and(|values| !values.is_empty()) {
            let mut command = Command::new(program);
            command.args(args);
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            if let Some(env) = env {
                command.envs(env);
            }
            configure_hidden_output_command(&mut command);
            command
                .output()
                .await
                .map_err(|e| AppError::Process(e.to_string()))?
        } else {
            output_command(program, args, cwd)
                .await
                .map_err(|e| AppError::Process(e.to_string()))?
        };
        if !output.status.success() {
            return Err(Self::command_failed(program, &output));
        }
        Ok(())
    }

    fn start_command(profile: &LaunchProfile) -> (&str, Vec<String>) {
        let program = &profile.command;
        let args = Self::rewrite_frontend_root_args_for_context(
            program,
            &Self::joined_args(&profile.args, profile.extra_args.as_deref()),
            profile.cwd.as_deref(),
        );
        (program, args)
    }

    fn restart_command(profile: &LaunchProfile) -> (&str, Vec<String>) {
        let program = profile.restart_command.as_deref().unwrap_or(&profile.command);
        let args = Self::rewrite_frontend_root_args_for_context(
            program,
            &Self::joined_args(
                profile.restart_args.as_deref().unwrap_or(&profile.args),
                profile.extra_args.as_deref(),
            ),
            profile.cwd.as_deref(),
        );
        (program, args)
    }

    pub async fn is_running(&self, installation_id: &str) -> AppResult<bool> {
        let mut map = self.inner.lock().await;
        let has_exited = match map.get_mut(installation_id) {
            Some(child) => Self::child_has_exited(child)?,
            None => return Ok(false),
        };
        if has_exited {
            let _ = map.remove(installation_id);
            return Ok(false);
        }
        Ok(true)
    }

    pub async fn shutdown_all(&self) {
        let children = {
            let mut map = self.inner.lock().await;
            map.drain().collect::<Vec<_>>()
        };
        for (_installation_id, mut child) in children {
            let _ = child.start_kill();
            let _ = timeout(STOP_WAIT_TIMEOUT, child.wait()).await;
        }
    }

    pub async fn start(&self, installation_id: &str, profile: &LaunchProfile) -> AppResult<()> {
        let mut map = self.inner.lock().await;
        let already_running = match map.get_mut(installation_id) {
            Some(child) => !Self::child_has_exited(child)?,
            None => false,
        };
        if already_running {
            return Err(AppError::Process(
                "process already running for installation".to_string(),
            ));
        }
        if map.contains_key(installation_id) {
            let _ = map.remove(installation_id);
        }
        let (program, args) = Self::start_command(profile);
        let child =
            Self::spawn_profile_command(program, &args, profile.cwd.as_deref(), profile.env.as_ref())?;
        map.insert(installation_id.to_string(), child);
        Ok(())
    }

    pub async fn force_stop(&self, installation_id: &str) -> AppResult<bool> {
        let mut map = self.inner.lock().await;
        let has_exited = match map.get_mut(installation_id) {
            Some(child) => Self::child_has_exited(child)?,
            None => return Ok(false),
        };
        if has_exited {
            let _ = map.remove(installation_id);
            return Ok(false);
        }
        let child = map
            .remove(installation_id)
            .ok_or_else(|| AppError::Process("process registry lost managed child".to_string()))?;
        drop(map);
        self.kill_and_wait_child(installation_id, child, "kill request")
            .await
    }

    pub async fn stop(&self, installation_id: &str, profile: &LaunchProfile) -> AppResult<bool> {
        let mut map = self.inner.lock().await;
        let has_exited = match map.get_mut(installation_id) {
            Some(child) => Self::child_has_exited(child)?,
            None => return Ok(false),
        };
        if has_exited {
            let _ = map.remove(installation_id);
            return Ok(false);
        }
        let mut child = map
            .remove(installation_id)
            .ok_or_else(|| AppError::Process("process registry lost managed child".to_string()))?;
        drop(map);
        if let Some(stop_command) = profile.stop_command.as_deref() {
            match timeout(
                STOP_HELPER_TIMEOUT,
                Self::run_profile_command(
                    stop_command,
                    profile.stop_args.as_deref().unwrap_or(&[]),
                    profile.cwd.as_deref(),
                    profile.env.as_ref(),
                ),
            )
            .await
            {
                Ok(Ok(())) => {
                    if Self::child_has_exited(&mut child)? {
                        return Ok(true);
                    }
                    return self
                        .kill_and_wait_child(installation_id, child, "successful stop command")
                        .await;
                }
                Ok(Err(err)) => {
                    self.reinsert_child(installation_id, child).await;
                    return Err(err);
                }
                Err(_) => {
                    return self
                        .kill_and_wait_child(installation_id, child, "stop command timeout")
                        .await;
                }
            }
        } else {
            return self
                .kill_and_wait_child(installation_id, child, "kill request")
                .await;
        }
    }

    pub async fn restart(&self, installation_id: &str, profile: &LaunchProfile) -> AppResult<()> {
        let stopped = self.stop(installation_id, profile).await?;
        if !stopped {
            return Err(AppError::Process(
                "process not running for installation".to_string(),
            ));
        }
        let (program, args) = Self::restart_command(profile);
        let mut map = self.inner.lock().await;
        let child = Self::spawn_profile_command(
            program,
            &args,
            profile.cwd.as_deref(),
            profile.env.as_ref(),
        )?;
        map.insert(installation_id.to_string(), child);
        Ok(())
    }
}

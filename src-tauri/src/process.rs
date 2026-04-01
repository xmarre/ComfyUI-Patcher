use crate::errors::{AppError, AppResult};
use crate::execution::{output_command, parse_wsl_unc_path, spawn_command};
use crate::models::LaunchProfile;
use std::collections::HashMap;
use std::path::Path;
use std::process::Output;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ProcessRegistry {
    inner: Arc<Mutex<HashMap<String, Child>>>,
}

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
        (
            &profile.command,
            Self::joined_args(&profile.args, profile.extra_args.as_deref()),
        )
    }

    fn restart_command(profile: &LaunchProfile) -> (&str, Vec<String>) {
        if let Some(command) = profile.restart_command.as_deref() {
            (
                command,
                Self::joined_args(
                    profile.restart_args.as_deref().unwrap_or(&[]),
                    profile.extra_args.as_deref(),
                ),
            )
        } else {
            (
                &profile.command,
                Self::joined_args(&profile.args, profile.extra_args.as_deref()),
            )
        }
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
        let mut child = map
            .remove(installation_id)
            .ok_or_else(|| AppError::Process("process registry lost managed child".to_string()))?;
        drop(map);
        child
            .start_kill()
            .map_err(|e| AppError::Process(e.to_string()))?;
        if let Err(err) = child.wait().await {
            let mut map = self.inner.lock().await;
            map.insert(installation_id.to_string(), child);
            return Err(AppError::Process(err.to_string()));
        }
        Ok(true)
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
            if let Err(err) = Self::run_profile_command(
                stop_command,
                profile.stop_args.as_deref().unwrap_or(&[]),
                profile.cwd.as_deref(),
                profile.env.as_ref(),
            )
            .await
            {
                let mut map = self.inner.lock().await;
                map.insert(installation_id.to_string(), child);
                return Err(err);
            }
        } else {
            child
                .start_kill()
                .map_err(|e| AppError::Process(e.to_string()))?;
        }
        if let Err(err) = child.wait().await {
            let mut map = self.inner.lock().await;
            map.insert(installation_id.to_string(), child);
            return Err(AppError::Process(err.to_string()));
        }
        Ok(true)
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

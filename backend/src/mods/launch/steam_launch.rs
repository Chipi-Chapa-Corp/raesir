use super::*;
use crate::mods::steam::{self, Request};

#[derive(Serialize, Deserialize)]
struct SteamSession {
    root: PathBuf,
    appid: u32,
    original: Option<String>,
    installed: String,
    #[serde(default)]
    profile: PathBuf,
    #[serde(default)]
    game_dir: PathBuf,
}

impl Manager {
    pub(super) fn launch_steam(
        &self,
        report: &Report,
        game: &LibraryGame,
        extra_args: &[String],
        mode: LaunchMode,
        lease: fs::File,
    ) -> Result<LaunchTask, String> {
        let slug = community(game).ok_or("Unknown mod community")?;
        let appid: u32 = game.id.parse().map_err(|_| "Invalid Steam app ID")?;
        let installation = game
            .installations
            .iter()
            .find(|i| i.os == Os::host() && matches!(i.state.as_str(), "installed" | "external"))
            .ok_or("The game is not installed on this OS")?;
        let game_dir = Path::new(&installation.path)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let root = wine_runtime::steam_client_root(Path::new(&installation.library))
            .ok_or("Could not find the owning Steam client")?;
        let profiles = self.profiles(slug);
        let profile = self.selected(slug, &profiles);
        if mode == LaunchMode::Modded
            && !profile.is_some_and(|p| p.installed.iter().any(|m| m.enabled))
        {
            return Err("This profile has no enabled mods".into());
        }
        let profile_path = if mode == LaunchMode::Vanilla {
            game_dir.as_path()
        } else {
            profile.map_or(game_dir.as_path(), |p| p.path.as_path())
        };
        let schema = ECOSYSTEM
            .game(slug)
            .and_then(|g| g.profile())
            .ok_or("No mod loader profile")?;
        let exe =
            launch::resolve_game_exe(&game_dir, &schema.exe_names).map_err(|e| e.to_string())?;
        let mut magic = [0; 2];
        fs::File::open(&exe)
            .and_then(|mut f| f.read_exact(&mut magic))
            .map_err(|e| e.to_string())?;
        let context = LaunchContext::new(
            HostOs::detect().ok_or("Unsupported OS")?,
            StorePlatform::Steam,
            if cfg!(target_os = "linux") && magic == *b"MZ" {
                Runtime::Proton
            } else {
                Runtime::Native
            },
            mode,
        )
        .with_extra_args(extra_args.to_vec());
        let plan = launch::launch_plan_in(profile_path, &ECOSYSTEM, slug, &context)
            .map_err(|e| e.to_string())?;
        let mut args = plan.args;
        if cfg!(target_os = "linux") {
            args.push(format!("--raesir-program={}", exe.display()));
        }
        if context.runtime == Runtime::Proton {
            args.push("--raesir-proton".into());
        }
        if mode == LaunchMode::Modded
            && let Some((_, value)) = plan.env.iter().find(|(k, _)| k == "WINEDLLOVERRIDES")
        {
            args.push(format!("--raesir-dll={value}"));
        }
        if std::env::var_os("ISOLATED_GUI_SESSION").is_some() {
            let env_file = PathBuf::from(
                std::env::var_os("RAESIR_MODS_TEST_ENV")
                    .ok_or("Steam GUI tests require an explicit private environment file")?,
            );
            let env_file = env_file.canonicalize().map_err(|e| e.to_string())?;
            if !env_file.starts_with(root.join("raesir/diagnostics")) {
                return Err(
                    "Steam test environment must be in the private diagnostic directory".into(),
                );
            }
            args.push(format!("--raesir-env={}", env_file.display()));
        }
        let staged = self.stage_dir(&game_dir);
        restore_session(&staged)?;
        if mode == LaunchMode::Modded {
            self.stage(game, profile_path, &game_dir, &context, &staged)?;
        }
        fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
        let result = if cfg!(target_os = "linux") {
            (|| {
                let wrapper = root.join("raesir/wrappers/launch.sh");
                write_wrapper(&wrapper)?;
                let original = steam::call(&root, Request::Read { appid })?;
                let installed = wrap_options(original.as_deref().unwrap_or(""), &wrapper)?;
                let session = SteamSession {
                    root: root.clone(),
                    appid,
                    original: original.clone(),
                    installed: installed.clone(),
                    profile: profile_path.into(),
                    game_dir: game_dir.clone(),
                };
                crate::storage::write_json(&staged.join("steam.json"), &session)?;
                steam::call(
                    &root,
                    Request::Set {
                        appid,
                        expected: original,
                        value: Some(installed),
                    },
                )?;
                steam::call(
                    &root,
                    Request::Launch {
                        appid,
                        args: quote_arguments(&args)?,
                    },
                )?;
                Ok::<_, String>(())
            })()
        } else {
            crate::actions::launch_with_args(report, &game.key, &args).map(|_| ())
        };
        if let Err(error) = result {
            restore_session(&staged)?;
            return Err(error);
        }
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(game_dir.clone());
        let active = self.active.clone();
        let operation = self.operation.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _lease = lease;
            let deadline = Instant::now() + Duration::from_secs(120);
            while !game_running(&game_dir) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(250));
            }
            let started = game_running(&game_dir);
            while game_running(&game_dir) {
                std::thread::sleep(Duration::from_secs(1));
            }
            let _guard = operation.lock().unwrap_or_else(|e| e.into_inner());
            let restored = restore_session(&staged);
            active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&game_dir);
            let result = restored.and_then(|_| {
                if started {
                    Ok(())
                } else {
                    Err(
                        "Steam did not start the modded game. Check the game in Steam, then retry."
                            .into(),
                    )
                }
            });
            let _ = tx.send(result);
        });
        Ok(LaunchTask(rx))
    }
}

pub(super) fn restore_session(staged: &Path) -> Result<(), String> {
    let file = staged.join("steam.json");
    let options = (|| {
        if file.exists() {
            let session: SteamSession =
                serde_json::from_slice(&fs::read(&file).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let current = steam::call(
                &session.root,
                Request::Read {
                    appid: session.appid,
                },
            )?;
            // If the user changed their options during play, keep their newer edit.
            if current.as_deref() == Some(&session.installed) {
                steam::call(
                    &session.root,
                    Request::Set {
                        appid: session.appid,
                        expected: current,
                        value: session.original,
                    },
                )?;
            }
            fs::remove_file(file).map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(())
    })();
    restore(staged)?;
    options?;
    if staged.exists() {
        fs::remove_dir_all(staged).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn wrap_options(original: &str, wrapper: &Path) -> Result<String, String> {
    if original.matches("%command%").count() > 1 {
        return Err("Steam launch options contain multiple %command% placeholders".into());
    }
    let wrapper = shlex::try_quote(wrapper.to_str().ok_or("Invalid wrapper path")?)
        .map_err(|e| e.to_string())?;
    Ok(if original.contains("%command%") {
        original.replacen("%command%", &format!("{wrapper} %command%"), 1)
    } else if original.trim().is_empty() {
        format!("{wrapper} %command%")
    } else {
        format!("{wrapper} %command% {original}")
    })
}

fn quote_arguments(args: &[String]) -> Result<String, String> {
    shlex::try_join(args.iter().map(String::as_str)).map_err(|e| e.to_string())
}

fn write_wrapper(path: &Path) -> Result<(), String> {
    let prefix = r#"#!/bin/sh
# Raesir's per-launch environment, followed by the shared engine wrapper.
RAESIR_REMAINING=$#
while [ "$RAESIR_REMAINING" -gt 0 ]; do
  RAESIR_ARGUMENT="$1"
  shift
  RAESIR_REMAINING=$((RAESIR_REMAINING - 1))
  case "$RAESIR_ARGUMENT" in
    --raesir-dll=*)
      WINEDLLOVERRIDES="${RAESIR_ARGUMENT#--raesir-dll=}${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
      export WINEDLLOVERRIDES
      ;;
    --raesir-env=*)
      RAESIR_ENV="${RAESIR_ARGUMENT#--raesir-env=}"
      [ -f "$RAESIR_ENV" ] || exit 78
      . "$RAESIR_ENV" || exit 78
      ;;
    *) set -- "$@" "$RAESIR_ARGUMENT" ;;
  esac
done
"#;
    let body = launch::wrapper_script_contents();
    let contents = format!(
        "{prefix}{}",
        body.strip_prefix("#!/bin/sh\n").unwrap_or(&body)
    );
    let parent = path.parent().ok_or("Wrapper path has no parent")?;
    let inner = parent.join("engine.sh");
    write_script(&inner, &contents)?;
    let inner = shlex::try_quote(inner.to_str().ok_or("Invalid wrapper path")?)
        .map_err(|e| e.to_string())?;
    // Bootstrap Steam's container/runtime first. Its launch client needs the
    // Steam session bus. Per-game environment belongs immediately before Proton
    // or the native game, after that bootstrap has finished.
    let outer = format!(
        r#"#!/bin/sh
RAESIR_INNER={inner}
RAESIR_PROGRAM=""
RAESIR_PROTON=""
RAESIR_REMAINING=$#
while [ "$RAESIR_REMAINING" -gt 0 ]; do
  RAESIR_ARGUMENT="$1"
  shift
  RAESIR_REMAINING=$((RAESIR_REMAINING - 1))
  case "$RAESIR_ARGUMENT" in
    --raesir-program=*) RAESIR_PROGRAM="${{RAESIR_ARGUMENT#--raesir-program=}}" ;;
    --raesir-proton) RAESIR_PROTON=1 ;;
    *) set -- "$@" "$RAESIR_ARGUMENT" ;;
  esac
done
[ -n "$RAESIR_PROGRAM" ] || exec "$@"
RAESIR_INSERTED=""
RAESIR_REMAINING=$#
while [ "$RAESIR_REMAINING" -gt 0 ]; do
  RAESIR_ARGUMENT="$1"
  shift
  RAESIR_REMAINING=$((RAESIR_REMAINING - 1))
  if [ -z "$RAESIR_INSERTED" ]; then
    if {{ [ -n "$RAESIR_PROTON" ] && [ "${{RAESIR_ARGUMENT##*/}}" = "proton" ]; }} || {{ [ -z "$RAESIR_PROTON" ] && [ "$RAESIR_ARGUMENT" -ef "$RAESIR_PROGRAM" ]; }}; then
      set -- "$@" "$RAESIR_INNER"
      RAESIR_INSERTED=1
    fi
  fi
  set -- "$@" "$RAESIR_ARGUMENT"
done
[ -n "$RAESIR_INSERTED" ] || exit 78
exec "$@"
"#
    );
    write_script(path, &outer)
}

fn write_script(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("Wrapper path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    fs::write(temp.path(), contents).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn wrapper_changes_environment_after_bootstrap_and_preserves_proton_argv() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("profile with spaces");
        let wrapper = root.join("launch.sh");
        write_wrapper(&wrapper).unwrap();
        let runtime = root.join("runtime");
        write_script(&runtime, "#!/bin/sh\n[ \"$DBUS_SESSION_BUS_ADDRESS\" = bootstrap-bus ] || exit 99\nexec \"$@\"\n").unwrap();
        let proton = root.join("proton");
        write_script(&proton, "#!/bin/sh\nprintf '%s\\n' \"$DBUS_SESSION_BUS_ADDRESS\" \"$WINEDLLOVERRIDES\" \"$@\"\n").unwrap();
        let env = root.join("private.env");
        fs::write(&env, "export DBUS_SESSION_BUS_ADDRESS=private-game-bus\n").unwrap();
        let output = Command::new(&wrapper)
            .arg(&runtime)
            .arg(&proton)
            .args([
                "run",
                "game path.exe",
                "two words",
                "quote'and\"literal",
                "$literal",
            ])
            .arg("--raesir-program=game path.exe")
            .arg("--raesir-proton")
            .arg("--raesir-dll=winhttp=n,b")
            .arg(format!("--raesir-env={}", env.display()))
            .env("DBUS_SESSION_BUS_ADDRESS", "bootstrap-bus")
            .env_remove("WINEDLLOVERRIDES")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "private-game-bus\nwinhttp=n,b\nrun\ngame path.exe\ntwo words\nquote'and\"literal\n$literal\n"
        );
        let refused = Command::new(&wrapper)
            .arg(&runtime)
            .arg("missing-proton")
            .arg("--raesir-program=game path.exe")
            .arg("--raesir-proton")
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(78));
        assert!(refused.stdout.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn native_wrapper_chainloads_the_shared_engine_loader_in_a_profile_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("wrapper/launch.sh");
        write_wrapper(&wrapper).unwrap();
        let game = dir.path().join("game path");
        write_script(&game, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        let profile = dir.path().join("mod profile");
        write_script(
            &profile.join("run_bepinex.sh"),
            "#!/bin/sh\nprintf 'loader initialized\\n'\nexec \"$@\"\n",
        )
        .unwrap();
        let output = Command::new(wrapper)
            .arg(&game)
            .arg("one argument")
            .arg(format!("--raesir-program={}", game.display()))
            .args(launch::wrapper_target_args(&profile))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "loader initialized\none argument\n"
        );
    }
    #[test]
    fn steam_wrapper_preserves_custom_environment_wrappers_and_game_arguments() {
        let path = Path::new("/profile with spaces/launch.sh");
        assert_eq!(
            wrap_options("MANGOHUD=1 gamemoderun %command% -windowed", path).unwrap(),
            "MANGOHUD=1 gamemoderun '/profile with spaces/launch.sh' %command% -windowed"
        );
        assert!(wrap_options("%command% %command%", path).is_err());
        assert_eq!(
            wrap_options("-windowed", path).unwrap(),
            "'/profile with spaces/launch.sh' %command% -windowed"
        );
    }
}

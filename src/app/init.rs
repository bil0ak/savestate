use super::*;

impl Savestate {
    pub fn open(root: PathBuf) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("canonicalize project root {}", root.display()))?;
        let config = config::Config::load(&root)?;
        let store = Store::open_strict(config.project_store(&root)?)?;
        Ok(Self {
            root,
            config,
            store,
        })
    }

    pub fn init(&mut self) -> Result<()> {
        self.init_with_guidance(true)
    }

    pub fn init_with_guidance(&mut self, show_next_steps: bool) -> Result<()> {
        let path = self.root.join(".savestate.toml");
        let already_initialized = path.is_file();
        let git_repository = self.root.join(".git").exists();
        let includes = detect_ignored_includes(&self.root)?;
        let postgres_detected = std::env::var_os("DATABASE_URL").is_some()
            || env_file_has_key(&self.root.join(".env"), "DATABASE_URL")?;
        if !path.exists() {
            let mut source = config::sample().to_owned();
            if !includes.is_empty() {
                let values = includes
                    .iter()
                    .map(|path| {
                        toml::Value::String(path.to_string_lossy().into_owned()).to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                source = source.replacen("include = []", &format!("include = [{values}]"), 1);
            }
            store::atomic_write(&path, source.as_bytes())?;
        } else if !includes.is_empty() {
            let source = fs::read_to_string(&path)?;
            let mut value: toml::Value = toml::from_str(&source)?;
            let table = value
                .as_table_mut()
                .context(".savestate.toml must contain a TOML table")?;
            let configured = table
                .entry("include")
                .or_insert_with(|| toml::Value::Array(Vec::new()))
                .as_array_mut()
                .context("include must be an array")?;
            let mut changed = false;
            for include in &includes {
                let include = include.to_string_lossy();
                if !configured
                    .iter()
                    .any(|value| value.as_str() == Some(&include))
                {
                    configured.push(toml::Value::String(include.into_owned()));
                    changed = true;
                }
            }
            if changed {
                store::atomic_write(&path, toml::to_string_pretty(&value)?.as_bytes())?;
            }
        }
        ensure_git_exclude(&self.root, self.store.path())?;
        ui::success(format_args!(
            "Savestate {}",
            if already_initialized {
                "configuration updated"
            } else {
                "initialized"
            }
        ));
        ui::field("Project", format_args!("{}", self.root.display()));
        ui::field("Store", format_args!("{}", self.store.path().display()));
        ui::success(format_args!("Snapshot scope configured"));
        if git_repository {
            ui::success(format_args!("Local store excluded from Git"));
        }
        if !includes.is_empty() {
            ui::line(format_args!(
                "Included ignored state: {}",
                includes
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for include in &includes {
            if has_sqlite_header(&self.root.join(include))? {
                ui::line(format_args!(
                    "Detected experimental SQLite candidate: {} (not enabled)",
                    include.display()
                ));
            }
        }
        if postgres_detected {
            ui::line(format_args!(
                "Detected experimental PostgreSQL candidate via DATABASE_URL (not enabled)"
            ));
        }
        if show_next_steps {
            ui::line(format_args!(""));
            ui::heading("Next");
            ui::line(format_args!(
                "  {}",
                ui::command("savestate integrate codex")
            ));
            ui::line(format_args!(
                "  {}",
                ui::command("savestate integrate claude")
            ));
        }
        Ok(())
    }
}

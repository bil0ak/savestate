pub fn sample() -> &'static str {
    r#"# Savestate project configuration
store = ".savestate"
keep_last = 20
respect_gitignore = true
include = []
exclude = []
external_paths = []

[limits]
warn_files = 100000
warn_size = "2GiB"

[retention]
manual = "keep"
agent = 20
recovery = 10
run = 10

[experimental]
databases = false

# Database adapters are experimental and require databases = true.
# [[sqlite]]
# path = "data/development.sqlite"

# [[postgres]]
# name = "development"
# url_env = "DATABASE_URL"
# maintenance_db = "postgres"
# allow_restore = false
# allow_remote_restore = false
"#
}

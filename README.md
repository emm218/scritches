# Scritches

I have some problems with existing solutions for scrobbling my music to last.fm
with MPD so I'm trying to do a rewrite

## Configuration

The config file is stored in `$XDG_CONFIG_HOME/scritches` by default and can use
any of the file formats supported by the
[config](https://crates.io/crates/config) crate (YAML, TOML, etc.) as long as
the part before the extension is `config`. The MPD hostname and port can also be
read from the `MPD_HOST` and `MPD_PORT` environment variables which will 
override the settings in the config file. Command line option have the highest
priority, overriding both environment variables and config files.

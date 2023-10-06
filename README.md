# Scritches

I have some problems with existing solutions for scrobbling my music to last.fm
with MPD so I'm trying to roll my own

## Configuration

The config file is stored in `$XDG_CONFIG_HOME/scritches` by default and can use
any of the file formats supported by the
[config](https://crates.io/crates/config) crate (YAML, TOML, etc.) as long as
the part before the extension is `config`. Currently the only options are
`mpd_hostname` and `mpd_port`. These can also be read from the `MPD_HOST` and
`MPD_PORT` environment variables which will override the settings in the config
file. Command line option have the highest priority, overriding both environment
variables and config files.

## Notes

Limitations on how MPD reports events make it non-trivial to tell when a song is
repeated. The logic used here works fine in the normal case of listening to a
song all the way through before restarting but breaks slightly in the case of
restarting a song over and over. I don't know why you'd do that though.

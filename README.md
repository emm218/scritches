# Scritches 🐱

I have some problems with existing solutions for scrobbling my music to last.fm
with MPD so I'm trying to roll my own.

## Features

- scrobble songs
- detects repeated songs properly
- update now playing status
- (un)love tracks through MPD client-to-client messages (e.g. 
  `mpc sendmessage scritches love` to love current track)
- save scrobbles and (un)loves to disk while unable to connect

## Configuration

The config file is stored in `$XDG_CONFIG_HOME/scritches` by default and can use
any of the file formats supported by the
[config](https://crates.io/crates/config) crate (YAML, TOML, etc.) as long as
the part before the extension is `config`.

Current config values:
- `mpd_addr` the TCP address to connect to MPD at (default `localhost:6600`)
- `mpd_socket` the path to a unix socket to connect to MPD at (default none)
- `mpd_password` the password to connect to MPD with (default none)
- `queue_path` file to log scrobbles in when offline (default
  `$XDG_STATE_HOME/scritches/queue`)
- `sk_path` file to persist session key in (default 
  `$XDG_STATE_HOME/scritches/sk`)
- `max_retry_time` the maximum time to take between retries in seconds (default
  960/16 mins)

any of these config values can also be set using command line options, which
will override the value read from the config file.

This application uses [env logger](https://crates.io/crates/env_logger) for
logging, so the log level defaults to `ERROR` (fatal problems or those which
require user intervention) and this can be changed with the `RUST_LOG`
environment variable.

## Notes

Limitations on how MPD reports events make it non-trivial to tell when a song is
repeated. The logic used here works fine in the normal case of listening to a
song all the way through before restarting but breaks slightly in the case of
restarting a song over and over. I don't know why you'd do that though.

## Todo

- sporadically panics due to an overflow on duration subtraction (can't reproduce)
- persist session key in dbus secrets service if available
- fix mysterious sporadic lockups when network dies

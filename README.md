# Scritches 🐱

I have some problems with existing solutions for scrobbling my music to last.fm
with MPD so I'm trying to roll my own

## Features

In addition to scrobbling, updating now playing status, and saving scrobbles
while offline, scritches lets you[^1] love or unlove tracks via messages on mpd's
client to client communication subsystem. Using `mpc` you can love the
current track with `mpc sendmessage scritches love` or unlove it with `mpc
sendmessage scritches unlove`. If scritches can't connect to last.fm then
these actions will be saved for later in the same way that scrobbles are.

[^1]: or, will let you once it's finished

## Configuration

The config file is stored in `$XDG_CONFIG_HOME/scritches` by default and can use
any of the file formats supported by the
[config](https://crates.io/crates/config) crate (YAML, TOML, etc.) as long as
the part before the extension is `config`.

Current config values:
- `mpd_addr` the TCP address to connect to MPD at (default `localhost:6600`)
- `mpd_socket` the path to a unix socket to connect to MPD at (default none)
- `mpd_password` the password to connect to MPD with (default none)
- `queue_path` the path to the file that should be used to log scrobbles when
  not connected to last.fm
- `max_retry_time` the maximum time to take between retries in seconds (default
  960/16 mins)

any of these config values can also be set using command line options, which
will override the value read from the config file.

## Notes

Limitations on how MPD reports events make it non-trivial to tell when a song is
repeated. The logic used here works fine in the normal case of listening to a
song all the way through before restarting but breaks slightly in the case of
restarting a song over and over. I don't know why you'd do that though.

## Todo

- handle talking to last.fm API

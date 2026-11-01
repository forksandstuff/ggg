# Godot Goodie Grabber

**Godot Goodie Grabber** (`ggg`) is a project manager for [Godot](https://godotengine.org/) games, inspired by [uv](https://github.com/astral-sh/uv) for Python.

Declare your Godot version and addon dependencies in a `ggg.toml` file. Then a single command sets up the full project environment - right engine version, all addons installed, every time.

```sh
ggg sync   # downloads Godot, installs all addons
ggg edit   # opens the project in the pinned editor
```

Addons can come from git repositories, pre-built archives, or the [Godot Asset Library](https://godotengine.org/asset-library/). A `ggg.lock` file records exact resolved versions so every contributor and every CI run gets a bit-for-bit identical environment.

## Documentation

**[godotneers.github.io/ggg](https://godotneers.github.io/ggg)**

## Development

- Integration tests in `tests/` run fully offline. Network endpoints (Godot manifest, Asset Library, Asset Store, GitHub releases) are mocked with wiremock and injected via the `GGG_*` environment variables in `src/envvars.rs`. Git dependencies use local file repositories built by `tests/common/git_fixtures.rs`.
- `.cargo/config.toml` caps build jobs at 4, because linking all integration test binaries in parallel can exhaust the Windows pagefile.
- `ggg.toml` is user-facing: keep schema changes backward compatible. Config is edited through `toml_edit` so user comments and formatting survive.
- The docs site in `docs/` is built with Zola and uses a git submodule for its theme (`git submodule update --init`). Run `docker compose up` in `docs/` to preview it locally.
- Releases are built by cargo-dist: pushing a version tag (e.g. `v0.5.0`) runs the release workflow.

## License

MIT OR Apache-2.0

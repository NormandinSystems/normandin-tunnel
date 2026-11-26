# Contributing

Thank you for looking. Bug reports, questions, translations and code are all
welcome.

## Reporting a bug or asking a question

[Open an issue](https://github.com/NormandinSystems/normandin-tunnel/issues).
Say what you did, what you expected, what happened instead, and which system
and version you use. Screenshots help.

* Never paste a connection code that is still in use.
* For a security problem, do not open an issue. Follow
  [SECURITY.md](SECURITY.md).

## Changing the code

1. Read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) to build and test.
2. For anything larger than a small fix, open an issue first so we can agree
   on the approach before you spend time on it.
3. Keep a pull request to one subject, and say in it what you changed and how
   you checked it.
4. Before you push:

   ```sh
   cargo fmt --all
   cargo clippy --all-targets -- -D warnings
   cargo test
   cd app && npm test
   ```

What we look for:

* **Tests come with the change.** A fix has a test that failed before it. A
  new message or limit has a test that sends too much of it.
* **Security-sensitive areas need extra care**: `tunnel-crypto`, the pairing
  and session code in `tunnel-core`, and `pathsafe.rs`. A change there should
  update [docs/PROTOCOL.md](docs/PROTOCOL.md) or
  [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) when it changes what they say.
* **No `unsafe`, and no new dependency without a reason.** CI checks
  dependency licences and advisories.
* **Every text the user sees goes in both** `app/src/i18n/en.json` **and**
  `fr.json`.
* **Plain words in the interface.** Write for someone who has never heard of
  encryption.

## Translations

The interface texts are in `app/src/i18n/`. To improve the French, edit
`fr.json`. To add a language, copy `en.json`, translate the values, and add
the language in `app/src/i18n/index.ts`.

## Licence of contributions

The project is licensed under either the Apache License 2.0 or the MIT
license, at the user's choice. Unless you say otherwise, anything you
contribute is offered under the same two licences, with no extra terms.

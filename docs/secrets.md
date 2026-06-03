# Bento Secret Store

Bento stores local secret material in a best-effort file-backed secret store. The default path is `${XDG_DATA_HOME}/bento/secrets.json`, or `~/.local/share/bento/secrets.json` when `XDG_DATA_HOME` is not set. The file is plain JSON today so the storage backend can stay small and replaceable.

The store file is a JSON object keyed by secret name. Secret names may contain ASCII letters, numbers, dots, underscores, and dashes. Names must not be empty, `.`, `..`, or start with `.`.

The initial file backend writes the parent directory with mode `0700` and the store file with mode `0600` on Unix platforms.

## CLI

The CLI manages raw secrets, not policy credentials. Use `bento secret login` for provider-backed OAuth secrets:

```sh
bento secret login openai-codex --name codex-personal
```

Use `bento secret set` for plain string secrets. Prefer stdin so the value does not end up in shell history:

```sh
printf '%s' "$TOKEN" | bento secret set github-token --value-stdin
```

For local throwaway values, `--value` is also available:

```sh
bento secret set something --value 123
```

## Schema

Each value has a `type` field. Supported types are:

- `plain`, a single string value for bearer-token style credentials.
- `oauth`, an OAuth token set with refresh metadata.

Example:

```json
{
  "codex-personal": {
    "type": "oauth",
    "access_token": "...",
    "refresh_token": "...",
    "expires_at": "2026-06-02T12:00:00Z",
    "account_id": "acct_123",
    "created_at": "2026-06-02T11:00:00Z",
    "updated_at": "2026-06-02T11:00:00Z"
  },
  "something": {
    "type": "plain",
    "value": "123"
  }
}
```

Policy credentials refer to secrets by name:

```hcl
credential "openai_codex_oauth" "personal" {
  endpoint = https.openai-codex
  secret = "codex-personal"
}
```

The netd implementation can read any referenced secret and can update only existing `oauth` secrets during token refresh. That keeps the current refresh path working while making a future read-only netd backend easier to introduce.

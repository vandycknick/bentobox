# Extensions

Bento extensions are optional guest features that Bento can enable and monitor for a VM.

Today, Bento supports these extensions:

- `ssh`
- `docker`

Extensions are guest-facing features. They are different from core VM functionality like the serial console or Bento's control API, which are always available.

## What Extensions Do

An extension lets Bento do three things for a VM:

- enable a guest feature for that VM
- check whether that feature is configured correctly
- report whether that feature is actually running

This means Bento can tell the difference between:

- a feature that is enabled but not installed correctly
- a feature that is installed but not started yet
- a feature that is fully ready

## Bootstrap And Userdata

Some Bento features require guest bootstrap. Bootstrap is how Bento injects cloud-init data into the guest.

Bootstrap is used automatically when needed. You do not need to enable it manually.

Bento enables bootstrap when:

- you provide `--userdata`
- you enable any extension that requires guest bootstrap

Your `userdata` remains regular cloud-init compatible user-data. Bento merges its own bootstrap content with your user-data automatically.

If an image does not support bootstrap, Bento will reject instance creation when bootstrap is required.

## Available Extensions

### `ssh`

The `ssh` extension enables Bento-managed SSH access to the guest.

What it means:

- Bento waits for SSH to become ready during `bentoctl start`
- `bentoctl shell <vm>` uses SSH by default when the `ssh` extension is enabled
- if SSH is not healthy yet, Bento keeps reporting the VM as not fully ready

This is a startup-required extension.

### `docker`

The `docker` extension enables Docker API access from the host to the guest.

What it means:

- Bento checks whether Docker is configured in the guest
- Bento checks whether Docker is actually running
- Bento exposes a per-instance host socket while the VM is running

This is not a startup-required extension.

That means:

- `bentoctl start` does not wait for Docker to become healthy
- Docker can still be reported as misconfigured or not running in `bentoctl status`

Current scope:

- rootful Docker only
- Docker must already be installed and configured in the guest image
- Bento does not install Docker for you yet

## Enabling Extensions

You enable extensions when creating a VM.

Example, enable Docker explicitly:

```bash
bentoctl create dev --image <image> --enable docker
```

You can enable more than one extension by repeating the flag:

```bash
bentoctl create dev --image <image> --enable ssh --enable docker
```

Notes:

- images may enable some extensions by default
- `ssh` may already be enabled depending on the image
- `docker` usually needs to be enabled explicitly unless the image defaults it on

## Starting A VM

Start the VM normally:

```bash
bentoctl start dev
```

Bento waits until the VM is considered ready.

Ready means:

- the VM itself is running
- the Bento guest agent is reachable
- all startup-required extensions are healthy

Today that mainly affects `ssh`.

So if `ssh` is enabled, `bentoctl start` waits for SSH to be ready before reporting success.

If `docker` is enabled but not healthy yet, startup can still succeed because Docker is not currently a startup-required extension.

## Using Docker

If the `docker` extension is enabled and healthy, Bento exposes a Docker-compatible Unix socket on the host for that VM.

You can inspect the socket path with:

```bash
bentoctl status dev
```

Example status output includes a `host sockets:` section like:

```text
host sockets:
  - docker => /path/to/instance/sock/docker.sock
```

You can use that socket with Docker clients by setting `DOCKER_HOST`:

```bash
export DOCKER_HOST=unix:///path/to/instance/sock/docker.sock
docker ps
```

The Docker socket only exists while the VM is running.

## Checking Status

Use:

```bash
bentoctl status dev
```

This shows:

- VM process state
- guest state
- overall readiness
- extension health
- exported host sockets

Example shape:

```text
name: dev
process: Running
vm: running
guest: running
ready: yes
extensions:
  - ssh enabled=true startup_required=true configured=true running=true
    summary: OpenSSH is configured and running
  - docker enabled=true startup_required=false configured=true running=false
    summary: Docker is configured but not running
    problem: Docker service is not reachable
host sockets:
  - docker => /.../sock/docker.sock
```

## What The Status Fields Mean

### `enabled`

Whether the extension is turned on for this VM.

### `startup_required`

Whether Bento waits for the extension during startup.

If this is `true`, `bentoctl start` does not report the VM healthy until the extension is ready.

If this is `false`, the extension is still checked and reported, but it does not block startup.

### `configured`

Whether the guest appears to be set up correctly for this extension.

Examples:

- `ssh` may be configured if OpenSSH is installed and expected config exists
- `docker` may be configured if Docker is installed in the guest

If `configured=false`, the guest is usually missing required software or setup.

### `running`

Whether the extension is currently reachable and working.

Examples:

- `ssh` is running when Bento can reach the SSH service in the guest
- `docker` is running when `/var/run/docker.sock` in the guest is responding

### `summary`

A short human-readable explanation of the current state.

### `problem`

Specific issues Bento detected for that extension.

## Common Scenarios

### Docker enabled, but not installed in the guest

`bentoctl status` may show:

- `enabled=true`
- `configured=false`
- `running=false`

This means Bento knows you want Docker, but the guest image does not currently satisfy the Docker requirements.

### Docker installed, but daemon not started

`bentoctl status` may show:

- `configured=true`
- `running=false`

This means Docker is present, but the daemon or socket is not ready yet.

### SSH enabled, startup still waiting

If `ssh` is enabled, Bento keeps startup in a not-ready state until SSH is healthy.

This is expected behavior.

## Choosing Images

Extensions depend on what the guest image supports.

A good image for extensions should:

- support Bento guest bootstrap
- support cloud-init compatible user-data
- already include the software needed by the extension, if required

For Docker specifically, the best current experience is an image that already has rootful Docker installed and configured.

## Current Limits

Current extension support is intentionally narrow:

- only built-in extensions are supported
- rootful Docker only
- Bento reports guest health, but does not provision Docker yet
- serial is not an extension and is always available separately

## Summary

Use extensions when you want Bento to manage and observe guest features.

In practice:

- use `ssh` for shell access and startup readiness
- use `docker` for per-VM Docker socket access
- use `bentoctl status` to understand what is enabled, configured, running, and exported

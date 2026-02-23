# TODOs

- [ ] Shell access hardening and UX
  - [ ] Use per-VM SSH keypairs instead of password auth.
  - [ ] Inject per-VM public key into guest via cloud-init at create/start time.
  - [ ] Configure guest sshd for key-based login and disable password auth.
  - [ ] Make `bentoctl shell <name>` use the per-VM private key automatically for instant login.

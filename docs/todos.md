# TODOs

- [ ] Replace hdiutil for cidata generate with something rust native
- [ ] Machine.Start synchronously waits until a machine is runnig atleast thats how it is implemented for now in the VZ backend. But thats not how Stop works, it's just a signal at the moment and doesn't wait for the underlying machine state. This means in the stop command there's some ugly polling logic. Maybe it's nicer if neither start and stop wait and are just triggers, and I expose a WatchState function from the Machine api.
- [ ]

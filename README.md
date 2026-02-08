# BentoBox 🍱

WORK IN PROGRESS

## Inspiration

- [macosvm](https://github.com/s-u/macosvm)
- [UTM](https://github.com/utmapp/UTM)
- [Lima]()

Idea:

- bento-runtime
    - Instance: Contains Instance, and knows how to create an instance based on conventions
    - InstanceManager

    - InstanceD
    - Driver

- bentoctl
    - I want this thing to just deal with defining the CLI and managing and validating arguments
    - Implements the Proc/Deamon trait that wires the upper command up to instance d starting up.
      at that point all what instancemanager does is calling spawn and read stdout to poll events
      for when the instance has started.

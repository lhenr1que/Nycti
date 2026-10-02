# Shell

The shell is Nycti's visual and interactive desktop surface. It presents system
state and sends user requests through documented APIs or IPC as a client; it
does not own system or window-management policy.

Caelestia is its primary upstream. Shell-specific changes must preserve a clear
boundary from the other Nycti components so the shell can be replaced
independently.

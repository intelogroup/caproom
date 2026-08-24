#!/usr/bin/env python3
"""pty_wrapper — allocate a pty and run command with verbatim forwarding, propagating exit code.

Usage: pty_wrapper.py <command> [args...]
"""
import os, sys, pty, select, signal, struct, fcntl, termios

def copy_winsize():
    try:
        s = struct.pack("HHHH", 0, 0, 0, 0)
        a = struct.unpack('hhhh', fcntl.ioctl(sys.stdout.fileno(), termios.TIOCGWINSZ, s))
        fcntl.ioctl(master_fd, termios.TIOCSWINSZ, struct.pack("HHHH", a[0], a[1], a[2], a[3]))
    except Exception:
        pass

if len(sys.argv) < 2:
    print("usage: pty_wrapper.py <command> [args...]", file=sys.stderr)
    sys.exit(1)

pid, master_fd = pty.fork()
if pid == 0:
    # child
    try:
        os.execvp(sys.argv[1], sys.argv[1:])
    except Exception as e:
        print(f"pty_wrapper: exec failed: {e}", file=sys.stderr)
        os._exit(127)

# parent: forward
# handle window resize
def handle_winch(sig, frame):
    copy_winsize()
try:
    signal.signal(signal.SIGWINCH, handle_winch)
except Exception:
    pass
copy_winsize()

# make stdin non-blocking if it's a tty
stdin_fd = sys.stdin.fileno()
stdout_fd = sys.stdout.fileno()
is_tty = os.isatty(stdin_fd)

# set master to non-blocking? select handles
import errno

exit_code = 127
try:
    while True:
        r, _, _ = select.select([master_fd] + ([stdin_fd] if is_tty else []), [], [], 0.1)
        if master_fd in r:
            try:
                data = os.read(master_fd, 1024)
            except OSError as e:
                if e.errno == errno.EIO:
                    data = b''
                else:
                    raise
            if not data:
                # EOF — child may have exited
                pass
            else:
                os.write(stdout_fd, data)
        if is_tty and stdin_fd in r:
            try:
                data = os.read(stdin_fd, 1024)
            except OSError:
                data = b''
            if data:
                os.write(master_fd, data)
            else:
                # stdin EOF
                pass
        # check if child exited
        try:
            pid2, status = os.waitpid(pid, os.WNOHANG)
            if pid2 != 0:
                if os.WIFEXITED(status):
                    exit_code = os.WEXITSTATUS(status)
                elif os.WIFSIGNALED(status):
                    exit_code = 128 + os.WTERMSIG(status)
                else:
                    exit_code = 1
                # drain remaining master data
                try:
                    while True:
                        r2, _, _ = select.select([master_fd], [], [], 0.1)
                        if not r2:
                            break
                        d = os.read(master_fd, 1024)
                        if not d:
                            break
                        os.write(stdout_fd, d)
                except Exception:
                    pass
                break
        except ChildProcessError:
            break
finally:
    try:
        os.close(master_fd)
    except Exception:
        pass
sys.exit(exit_code)

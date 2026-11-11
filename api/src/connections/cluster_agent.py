#!/usr/bin/env python3
"""Chat's file session on a cluster.

Runs as the person inside a Slurm job Chat submitted. It connects out to Chat, asks
for the chat's file operations one at a time, carries each out under the roots it was
given, and posts the result. Nothing listens. It exits once idle, when the job ends,
or when Chat ends the session. Standard library only, Python 3.6 or newer.

To try one operation by hand on a cluster:
  OBLETH_CHAT_ROOTS='~,/scratch/$USER' python3 cluster_agent.py --op list --args '{"path":"~"}'
"""
import datetime
import fnmatch
import getpass
import json
import os
import re
import shutil
import signal
import socket
import sys
import time
import urllib.error
import urllib.request

VERSION = 1
LIST_LIMIT = 500
FIND_LIMIT = 500
SEARCH_LIMIT = 200
USAGE_LIMIT = 50
REMOVE_LIMIT = 200
READ_DEFAULT = 48 * 1024
READ_LIMIT = 256 * 1024
WRITE_LIMIT = 1024 * 1024
SEARCH_FILE_LIMIT = 8 * 1024 * 1024
WALK_SECONDS = 60
USAGE_SECONDS = 120
OUTPUT_LIMIT = 320 * 1024
LINE_LIMIT = 300

HOME = os.path.realpath(os.path.expanduser("~"))
TRASH = os.path.join(HOME, ".chat-trash")


class Refused(Exception):
    """An operation the session will not carry out, with the reason for the model."""


def expand(text):
    text = text.strip().replace("${USER}", getpass.getuser()).replace("$USER", getpass.getuser())
    return os.path.expanduser(text)


def roots():
    out = []
    for part in os.environ.get("OBLETH_CHAT_ROOTS", "~").split(","):
        if part.strip():
            out.append(os.path.realpath(expand(part)))
    return out or [HOME]


ROOTS = roots()


def shown_roots():
    return ", ".join("~" if r == HOME else r for r in ROOTS)


def resolve(path, must_exist=True):
    """The real path, when it lies under one of the roots."""
    if not isinstance(path, str) or not path.strip():
        raise Refused("path is required")
    given = path.strip()
    p = expand(given)
    if not os.path.isabs(p):
        p = os.path.join(HOME, p)
    real = os.path.realpath(p)
    for root in ROOTS:
        if real == root or real.startswith(root + os.sep):
            if must_exist and not os.path.lexists(real):
                raise Refused("There is no %s" % given)
            return real
    raise Refused("%s is outside what Chat may reach here: %s" % (given, shown_roots()))


def human(size):
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if size < 1024 or unit == "TB":
            return "%d %s" % (size, unit) if unit == "B" else "%.1f %s" % (size, unit)
        size /= 1024.0
    return "%d B" % size


def when(seconds):
    return datetime.datetime.fromtimestamp(seconds).strftime("%Y-%m-%d %H:%M")


def entry_kind(entry):
    if entry.is_symlink():
        return "link"
    if entry.is_dir(follow_symlinks=False):
        return "dir"
    return "file"


def integer(args, name, default, low, high):
    value = args.get(name, default)
    if value is None:
        return default
    try:
        value = int(value)
    except (TypeError, ValueError):
        raise Refused("%s must be a number" % name)
    return max(low, min(high, value))


def clip_line(text):
    text = text.rstrip("\r\n")
    return text if len(text) <= LINE_LIMIT else text[:LINE_LIMIT] + "…"


def walk(top, deadline):
    """Every directory under top, without following links, until the deadline."""
    for root, dirs, files in os.walk(top, topdown=True, onerror=lambda e: None, followlinks=False):
        if time.time() > deadline:
            return
        dirs.sort()
        files.sort()
        yield root, dirs, files


# Operations. Each takes the arguments the model gave and returns the text it reads.


def op_list(args):
    path = resolve(args.get("path", "~"))
    hidden = bool(args.get("hidden", False))
    if not os.path.isdir(path):
        st = os.lstat(path)
        return "%s is a %s of %s, modified %s" % (path, "link" if os.path.islink(path) else "file", human(st.st_size), when(st.st_mtime))
    entries = []
    with os.scandir(path) as it:
        for entry in it:
            if not hidden and entry.name.startswith("."):
                continue
            try:
                st = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            entries.append((entry_kind(entry), entry.name, st.st_size, st.st_mtime))
    entries.sort(key=lambda e: (e[0] != "dir", e[1].lower()))
    lines = ["%s (%d entries%s):" % (path, len(entries), "" if hidden else ", hidden ones left out")]
    for kind, name, size, mtime in entries[:LIST_LIMIT]:
        if kind == "dir":
            lines.append("dir   %10s  %s  %s/" % ("", when(mtime), name))
        else:
            lines.append("%-5s %10s  %s  %s" % (kind, human(size), when(mtime), name))
    if len(entries) > LIST_LIMIT:
        lines.append("… and %d more; name a narrower path or use find" % (len(entries) - LIST_LIMIT))
    return "\n".join(lines)


def op_find(args):
    path = resolve(args.get("path", "~"))
    name = args.get("name") or "*"
    larger = args.get("larger_than_mb")
    older = args.get("older_than_days")
    limit = integer(args, "limit", FIND_LIMIT, 1, FIND_LIMIT)
    min_size = float(larger) * 1024 * 1024 if larger not in (None, "") else None
    before = time.time() - float(older) * 86400 if older not in (None, "") else None
    deadline = time.time() + WALK_SECONDS
    found = []
    for root, dirs, files in walk(path, deadline):
        for item in dirs + files:
            if not fnmatch.fnmatch(item, name):
                continue
            full = os.path.join(root, item)
            try:
                st = os.lstat(full)
            except OSError:
                continue
            if min_size is not None and st.st_size < min_size:
                continue
            if before is not None and st.st_mtime > before:
                continue
            found.append((full, st.st_size, st.st_mtime))
            if len(found) >= limit:
                break
        if len(found) >= limit:
            break
    cut = time.time() > deadline
    lines = ["%d match%s under %s%s%s:" % (len(found), "" if len(found) == 1 else "es", path, " (the first %d)" % limit if len(found) >= limit else "", "; the search stopped at its %d s limit" % WALK_SECONDS if cut else "")]
    for full, size, mtime in found:
        lines.append("%10s  %s  %s" % (human(size), when(mtime), full))
    return "\n".join(lines)


def op_read(args):
    path = resolve(args.get("path"))
    if os.path.isdir(path):
        raise Refused("%s is a directory; list it instead" % path)
    offset = integer(args, "offset", 0, 0, 1 << 50)
    most = integer(args, "max_bytes", READ_DEFAULT, 1, READ_LIMIT)
    total = os.path.getsize(path)
    with open(path, "rb") as f:
        f.seek(offset)
        data = f.read(most)
    if b"\0" in data[:8192]:
        return "%s is a binary file of %s" % (path, human(total))
    end = offset + len(data)
    text = data.decode("utf-8", "replace")
    head = "%s (%s" % (path, human(total))
    if offset or end < total:
        head += ", bytes %d–%d" % (offset, end)
    if end < total:
        head += "; more follows at offset %d" % end
    return head + "):\n" + text


def op_search(args):
    path = resolve(args.get("path", "~"))
    pattern = args.get("pattern")
    if not isinstance(pattern, str) or not pattern:
        raise Refused("pattern is required")
    try:
        regex = re.compile(pattern)
    except re.error as error:
        raise Refused("pattern is not a valid regular expression: %s" % error)
    name = args.get("name") or "*"
    limit = integer(args, "limit", SEARCH_LIMIT, 1, SEARCH_LIMIT)
    deadline = time.time() + WALK_SECONDS
    hits = []
    files_seen = 0

    def search_file(full):
        nonlocal files_seen
        try:
            if os.path.getsize(full) > SEARCH_FILE_LIMIT:
                return
            with open(full, "rb") as f:
                head = f.read(8192)
                if b"\0" in head:
                    return
                f.seek(0)
                files_seen += 1
                for number, line in enumerate(f, 1):
                    if regex.search(line.decode("utf-8", "replace")):
                        hits.append("%s:%d: %s" % (full, number, clip_line(line.decode("utf-8", "replace"))))
                        if len(hits) >= limit:
                            return
        except OSError:
            return

    if os.path.isfile(path):
        search_file(path)
    else:
        for root, dirs, files in walk(path, deadline):
            for item in files:
                if fnmatch.fnmatch(item, name):
                    search_file(os.path.join(root, item))
                if len(hits) >= limit or time.time() > deadline:
                    break
            if len(hits) >= limit or time.time() > deadline:
                break
    cut = time.time() > deadline
    head = "%d line%s under %s%s%s:" % (len(hits), " matches in %d file%s" % (files_seen, "" if files_seen == 1 else "s") if len(hits) == 1 else "s match in %d file%s" % (files_seen, "" if files_seen == 1 else "s"), path, " (the first %d)" % limit if len(hits) >= limit else "", "; the search stopped at its %d s limit" % WALK_SECONDS if cut else "")
    return "\n".join([head] + hits)


def op_usage(args):
    path = resolve(args.get("path", "~"))
    if not os.path.isdir(path):
        return "%s is a file of %s" % (path, human(os.path.getsize(path)))
    deadline = time.time() + USAGE_SECONDS
    sizes = []
    with os.scandir(path) as it:
        children = sorted(it, key=lambda e: e.name)
    cut = False
    for child in children:
        try:
            st = child.stat(follow_symlinks=False)
        except OSError:
            continue
        size = st.st_size
        if child.is_dir(follow_symlinks=False):
            for root, dirs, files in walk(child.path, deadline):
                for item in files:
                    try:
                        size += os.lstat(os.path.join(root, item)).st_size
                    except OSError:
                        pass
            if time.time() > deadline:
                cut = True
        sizes.append((size, child.name + ("/" if child.is_dir(follow_symlinks=False) else "")))
    sizes.sort(reverse=True)
    total = sum(s for s, _ in sizes)
    lines = ["%s holds %s in files%s:" % (path, human(total), " (counting stopped at its %d s limit, so this is a lower bound)" % USAGE_SECONDS if cut else "")]
    for size, name in sizes[:USAGE_LIMIT]:
        lines.append("%10s  %s" % (human(size), name))
    if len(sizes) > USAGE_LIMIT:
        lines.append("… and %d smaller entries" % (len(sizes) - USAGE_LIMIT))
    return "\n".join(lines)


def op_write(args):
    path = resolve(args.get("path"), must_exist=False)
    content = args.get("content", "")
    if not isinstance(content, str):
        raise Refused("content must be text")
    data = content.encode("utf-8")
    if len(data) > WRITE_LIMIT:
        raise Refused("content is %s; a write is at most %s" % (human(len(data)), human(WRITE_LIMIT)))
    if os.path.isdir(path):
        raise Refused("%s is a directory" % path)
    existed = os.path.lexists(path)
    if existed and not args.get("overwrite", False):
        raise Refused("%s exists; set overwrite to replace it" % path)
    parent = os.path.dirname(path)
    os.makedirs(parent, exist_ok=True)
    temp = os.path.join(parent, ".chat-write-%d.tmp" % os.getpid())
    with open(temp, "wb") as f:
        f.write(data)
    os.replace(temp, path)
    return "%s %s (%s)" % ("Replaced" if existed else "Wrote", path, human(len(data)))


def guard_root(path, what):
    if path in ROOTS or path == HOME:
        raise Refused("%s is a root Chat may reach, and is not %s" % (path, what))
    if path == TRASH or path.startswith(TRASH + os.sep):
        raise Refused("%s is in the trash" % path)


def op_move(args):
    source = resolve(args.get("from"))
    target = resolve(args.get("to"), must_exist=False)
    guard_root(source, "moved")
    if os.path.isdir(target):
        target = os.path.join(target, os.path.basename(source))
    if os.path.lexists(target):
        raise Refused("%s exists" % target)
    os.makedirs(os.path.dirname(target), exist_ok=True)
    shutil.move(source, target)
    return "Moved %s to %s" % (source, target)


def op_remove(args):
    paths = args.get("paths")
    if not isinstance(paths, list) or not paths:
        raise Refused("paths must list what to move to the trash")
    if len(paths) > REMOVE_LIMIT:
        raise Refused("at most %d paths at a time" % REMOVE_LIMIT)
    folder = os.path.join(TRASH, datetime.datetime.now().strftime("%Y-%m-%d-%H%M"))
    lines = []
    for given in paths:
        path = resolve(given)
        guard_root(path, "moved to the trash")
        target = folder + path
        os.makedirs(os.path.dirname(target), exist_ok=True)
        if os.path.lexists(target):
            target = "%s.%d" % (target, int(time.time()))
        shutil.move(path, target)
        lines.append("%s → %s" % (path, target))
    lines.append("Moved %d to the trash under %s. They can be moved back from there." % (len(lines), folder))
    return "\n".join(lines)


OPS = {
    "list": op_list,
    "find": op_find,
    "read": op_read,
    "search": op_search,
    "usage": op_usage,
    "write": op_write,
    "move": op_move,
    "remove": op_remove,
}


def run(op, args):
    """(ok, output) for one operation; errors are text for the model, never a crash."""
    try:
        if op not in OPS:
            raise Refused("there is no file operation named %s" % op)
        if not isinstance(args, dict):
            raise Refused("arguments must be an object")
        output = OPS[op](args)
    except Refused as error:
        return False, str(error)
    except Exception as error:  # pylint: disable=broad-except
        return False, "%s: %s" % (type(error).__name__, error)
    if len(output.encode("utf-8")) > OUTPUT_LIMIT:
        output = output.encode("utf-8")[:OUTPUT_LIMIT].decode("utf-8", "ignore") + "\n… (cut at %s)" % human(OUTPUT_LIMIT)
    return True, output


# The line to Chat.


class Chat:
    def __init__(self, url, token):
        self.url = url.rstrip("/")
        self.token = token

    def request(self, method, path, body=None, timeout=40):
        data = None if body is None else json.dumps(body).encode("utf-8")
        req = urllib.request.Request(self.url + path, data=data, method=method)
        req.add_header("Authorization", "Bearer " + self.token)
        req.add_header("Content-Type", "application/json")
        req.add_header("User-Agent", "obleth-chat-cluster-agent/%d" % VERSION)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.read()

    def bye(self, reason):
        try:
            self.request("POST", "/api/cluster-sessions/bye", {"reason": reason}, timeout=10)
        except (urllib.error.URLError, OSError):
            pass


def log(text):
    sys.stderr.write("%s %s\n" % (datetime.datetime.now().strftime("%H:%M:%S"), text))
    sys.stderr.flush()


def serve():
    url = os.environ.get("OBLETH_CHAT_URL", "")
    token = os.environ.get("OBLETH_CHAT_SESSION", "")
    if not url or not token:
        log("OBLETH_CHAT_URL and OBLETH_CHAT_SESSION are needed")
        return 2
    idle = float(os.environ.get("OBLETH_CHAT_IDLE", "3600"))
    chat = Chat(url, token)

    def ended(signum, frame):
        chat.bye("the job was ended")
        sys.exit(143)

    signal.signal(signal.SIGTERM, ended)
    signal.signal(signal.SIGINT, ended)
    hello = {"username": getpass.getuser(), "uid": os.getuid(), "host": socket.gethostname(), "version": VERSION, "roots": ROOTS}
    try:
        status, body = chat.request("POST", "/api/cluster-sessions/hello", hello)
    except (urllib.error.URLError, OSError) as error:
        log("Chat at %s is not reachable from this node: %s" % (url, error))
        return 1
    if status != 200:
        log("Chat refused the session (HTTP %d): %s" % (status, body.decode("utf-8", "replace")[:300]))
        return 1
    try:
        idle = float(json.loads(body.decode("utf-8")).get("idle", idle))
    except (ValueError, AttributeError):
        pass
    last = time.time()
    failures = 0
    while True:
        if time.time() - last > idle:
            chat.bye("idle for %d minutes" % (idle // 60))
            return 0
        try:
            status, body = chat.request("GET", "/api/cluster-sessions/next")
            failures = 0
        except (urllib.error.URLError, OSError, socket.timeout) as error:
            failures += 1
            if failures > 60:
                log("Chat stopped answering: %s" % error)
                return 1
            time.sleep(10)
            continue
        if status == 204:
            continue
        if status in (401, 410):
            return 0
        if status != 200:
            time.sleep(5)
            continue
        try:
            call = json.loads(body.decode("utf-8"))
        except ValueError:
            continue
        last = time.time()
        ok, output = run(call.get("op"), call.get("arguments") or {})
        try:
            chat.request("POST", "/api/cluster-sessions/calls/%s" % call.get("id"), {"ok": ok, "output": output})
        except (urllib.error.URLError, OSError) as error:
            log("could not post a result: %s" % error)


def main(argv):
    if "--op" in argv:
        op = argv[argv.index("--op") + 1]
        args = json.loads(argv[argv.index("--args") + 1]) if "--args" in argv else {}
        ok, output = run(op, args)
        print(output)
        return 0 if ok else 1
    return serve()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

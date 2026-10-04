#!/usr/bin/env python3
"""Deterministic fake LSP stdio server for automated tests.

Usage: fake-lsp-server.py <mode> [args]

Modes:
  success            proper handshake; echoes unknown methods back as results
  unsupported        every request gets a -32601 MethodNotFound error
  delayed <ms>       success, but each response is delayed by <ms>
  crash [n]          read n messages (default 1) then die without replying
  malformed          emit one non-LSP frame then die
  oversized          announce a >16 MiB Content-Length body then die
  noexit             success, but the `exit` notification is ignored forever
  apply-edit         success, then send the client a workspace/applyEdit
                     request (id 9001) and report its reply via a
                     `custom/serverSawReply` notification
  sync <file>        incremental document sync (utf-16, change:2, save with
                     text); applies didOpen/didChange/didClose to its own
                     document table and dumps {"log", "docs"} JSON to <file>
                     on exit/EOF

The script writes nothing to stdout that is not a protocol frame; progress
noise goes to stderr. No network, no filesystem mutation.
"""

import json
import os
import sys
import time


def read_exact(r_in, count):
    data = b""
    while len(data) < count:
        chunk = r_in.read(count - len(data))
        if not chunk:
            return None
        data += chunk
    return data


def read_message(r_in):
    """Read one framed message. Returns the body bytes or None at EOF."""
    length = None
    while True:
        line = r_in.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length":
            length = int(value.strip())
    if length is None:
        return None
    return read_exact(r_in, length)


def write_message(w_out, obj):
    body = json.dumps(obj).encode("utf-8")
    w_out.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    w_out.flush()


def eprint(*args):
    print(*args, file=sys.stderr, flush=True)


def is_request(msg):
    return "id" in msg and "method" in msg


def handle_request(msg, mode_opts):
    method = msg.get("method")
    if method == "initialize":
        capabilities = {
            "positionEncoding": "utf-16",
            "textDocumentSync": mode_opts.get("sync", 1),
        }
        # Tests advertise feature providers (and any other capability)
        # through the argv JSON's "capabilities" map.
        capabilities.update(mode_opts.get("capabilities", {}))
        return {
            "jsonrpc": "2.0",
            "id": msg["id"],
            "result": {"capabilities": capabilities},
        }
    if method == "shutdown":
        return {"jsonrpc": "2.0", "id": msg["id"], "result": None}
    # Canned feature responses: {"features": {"textDocument/hover": {...}}}
    # — an entry may be a plain result body or {"error": {...}} to exercise
    # the error path.
    features = mode_opts.get("features", {})
    if method in features:
        canned = features[method]
        if isinstance(canned, dict) and "error" in canned:
            return {"jsonrpc": "2.0", "id": msg["id"], "error": canned["error"]}
        return {"jsonrpc": "2.0", "id": msg["id"], "result": canned}
    if mode_opts.get("echo_result"):
        return {"jsonrpc": "2.0", "id": msg["id"], "result": {"echo": method}}
    return {"jsonrpc": "2.0", "id": msg["id"], "result": {"ok": True, "method": method}}


def run_success(r_in, w_out, mode_opts):
    delay = float(mode_opts.get("delay_ms", 0)) / 1000.0
    while True:
        body = read_message(r_in)
        if body is None:
            return 0
        try:
            msg = json.loads(body)
        except ValueError:
            eprint("fake-lsp-server: undecodable body", body[:80])
            return 2
        if is_request(msg):
            reply = handle_request(msg, mode_opts)
            if delay:
                time.sleep(delay)
            write_message(w_out, reply)
        elif msg.get("method") == "exit":
            if mode_opts.get("ignore_exit"):
                # Keep running: a bounded client must kill us.
                time.sleep(3600)
                return 0
            return 0


def run_unsupported(r_in, w_out):
    while True:
        body = read_message(r_in)
        if body is None:
            return 0
        try:
            msg = json.loads(body)
        except ValueError:
            return 2
        if is_request(msg):
            write_message(
                w_out,
                {
                    "jsonrpc": "2.0",
                    "id": msg["id"],
                    "error": {"code": -32601, "message": "unsupported here"},
                },
            )
        elif msg.get("method") == "exit":
            return 0


def run_crash(r_in, read_count):
    for _ in range(read_count):
        if read_message(r_in) is None:
            break
    os._exit(1)  # die mid-protocol: no reply, no cleanup, like a real crash


def run_malformed(w_out):
    w_out.write(b"this is not an LSP frame\r\n")
    w_out.write(b"Content-Length: notanumber\r\n\r\n")
    w_out.flush()
    return 1


def run_oversized(w_out):
    # Declare a body larger than the client's cap; the decoder must reject on
    # the header alone, so the bytes never need to arrive.
    w_out.write(b"Content-Length: 20000000\r\n\r\n")
    w_out.write(b"{")
    w_out.flush()
    return 1


def utf16_col_to_index(line, col):
    """UTF-16 code-unit column -> Python str index (surrogates count 2)."""
    units = 0
    for i, ch in enumerate(line):
        if units == col:
            return i
        units += 2 if ord(ch) > 0xFFFF else 1
    return len(line) if units == col else None


def apply_change(text, change):
    """Apply one TextDocumentContentChangeEvent (utf-16 positions)."""
    if "range" not in change:
        return change["text"]
    lines = text.split("\n")
    rng = change["range"]
    sl, sc = rng["start"]["line"], rng["start"]["character"]
    el, ec = rng["end"]["line"], rng["end"]["character"]
    if sl >= len(lines) or el >= len(lines):
        return text  # out-of-range: leave mirror untouched (client bug bait)
    start = sum(len(l) + 1 for l in lines[:sl]) + (utf16_col_to_index(lines[sl], sc) or 0)
    end = sum(len(l) + 1 for l in lines[:el]) + (utf16_col_to_index(lines[el], ec) or 0)
    return text[:start] + change["text"] + text[end:]


def run_sync(r_in, w_out, transcript_path):
    """Full-sync server: incremental apply + transcript dump for tests."""
    docs = {}
    # A restarted server continues the transcript log (its document table
    # starts empty — the client must re-open every doc).
    log = []
    if os.path.exists(transcript_path):
        try:
            with open(transcript_path) as f:
                log = json.load(f).get("log", [])
        except (ValueError, OSError):
            log = []
    opts = {
        "echo_result": True,
        "sync": {"openClose": True, "change": 2, "save": {"includeText": True}},
    }
    # argv[3] (optional): {"capabilities": {...}, "features": {...}}
    if len(sys.argv) > 3 and sys.argv[3]:
        try:
            opts.update(json.loads(sys.argv[3]))
        except ValueError:
            eprint("fake-lsp-server: bad argv JSON", sys.argv[3][:80])

    # Persist after EVERY message via atomic replace: a client that kills us
    # (SIGKILL after the bounded shutdown deadline, or EOF on restart) must
    # still leave a complete transcript — a torn write reads as none at all.
    def flush(reason):
        tmp = transcript_path + ".tmp"
        with open(tmp, "w") as f:
            json.dump({"reason": reason, "log": log, "docs": docs}, f)
        os.replace(tmp, transcript_path)

    while True:
        body = read_message(r_in)
        if body is None:
            flush("eof")
            return 0
        try:
            msg = json.loads(body)
        except ValueError:
            flush("bad-json")
            return 2
        if is_request(msg):
            # Requests land in the same transcript (flagged), then flush —
            # tests can prove a request crossed the pipe verbatim.
            log.append({"method": msg.get("method"), "id": msg.get("id"), "request": True})
            write_message(w_out, handle_request(msg, opts))
            flush("live")
            continue
        method = msg.get("method")
        if method == "exit":
            flush("exit")
            return 0
        params = msg.get("params") or {}
        td = params.get("textDocument") or {}
        uri = td.get("uri")
        if method == "textDocument/didOpen":
            docs[uri] = td.get("text", "")
        elif method == "textDocument/didChange":
            for ch in params.get("contentChanges", []):
                docs[uri] = apply_change(docs.get(uri, ""), ch)
        elif method == "textDocument/didClose":
            docs.pop(uri, None)
        log.append({"method": method, "uri": uri, "version": td.get("version")})
        flush("live")


def run_apply_edit(r_in, w_out):
    body = read_message(r_in)
    if body is None:
        return 1
    msg = json.loads(body)
    if msg.get("method") == "initialize":
        write_message(
            w_out,
            {
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {"capabilities": {"positionEncoding": "utf-16"}},
            },
        )
    # Server-driven workspace mutation attempt: the client MUST answer it
    # explicitly (never silently apply).
    write_message(
        w_out,
        {
            "jsonrpc": "2.0",
            "id": 9001,
            "method": "workspace/applyEdit",
            "params": {"edit": {"changes": {"/etc/passwd": []}}},
        },
    )
    while True:
        body = read_message(r_in)
        if body is None:
            return 1
        try:
            msg = json.loads(body)
        except ValueError:
            return 2
        if "id" in msg and msg.get("id") == 9001 and "method" not in msg:
            error = msg.get("error") or {}
            write_message(
                w_out,
                {
                    "jsonrpc": "2.0",
                    "method": "custom/serverSawReply",
                    "params": {
                        "replied": True,
                        "code": error.get("code"),
                    },
                },
            )
            return 0


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "success"
    r_in = sys.stdin.buffer
    w_out = sys.stdout.buffer

    if mode == "success":
        sys.exit(run_success(r_in, w_out, {"echo_result": True}))
    if mode == "unsupported":
        sys.exit(run_unsupported(r_in, w_out))
    if mode == "delayed":
        delay_ms = sys.argv[2] if len(sys.argv) > 2 else "0"
        sys.exit(
            run_success(
                r_in,
                w_out,
                {"echo_result": True, "delay_ms": delay_ms},
            )
        )
    if mode == "crash":
        count = int(sys.argv[2]) if len(sys.argv) > 2 else 1
        sys.exit(run_crash(r_in, count))
    if mode == "malformed":
        sys.exit(run_malformed(w_out))
    if mode == "oversized":
        sys.exit(run_oversized(w_out))
    if mode == "noexit":
        sys.exit(run_success(r_in, w_out, {"echo_result": True, "ignore_exit": True}))
    if mode == "sync":
        transcript = sys.argv[2] if len(sys.argv) > 2 else "/tmp/fake-lsp-sync.json"
        sys.exit(run_sync(r_in, w_out, transcript))
    if mode == "apply-edit":
        sys.exit(run_apply_edit(r_in, w_out))
    eprint("fake-lsp-server: unknown mode", mode)
    sys.exit(64)


if __name__ == "__main__":
    main()

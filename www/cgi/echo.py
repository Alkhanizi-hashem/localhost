#!/usr/bin/env python3
import os
import sys

body = sys.stdin.buffer.read()

print("Content-Type: text/html; charset=utf-8")
print()
print("<!doctype html>")
print("<html><body>")
print("<h1>CGI echo</h1>")
print("<dl>")
print(f"<dt>method</dt><dd>{os.environ.get('REQUEST_METHOD', '')}</dd>")
print(f"<dt>query</dt><dd>{os.environ.get('QUERY_STRING', '')}</dd>")
print(f"<dt>path_info</dt><dd>{os.environ.get('PATH_INFO', '')}</dd>")
print(f"<dt>body bytes</dt><dd>{len(body)}</dd>")
print("</dl>")
if body:
    print("<pre>")
    print(body.decode("utf-8", "replace"))
    print("</pre>")
print("</body></html>")

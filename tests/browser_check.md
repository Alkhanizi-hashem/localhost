# Browser Check

Use a current Chrome or Firefox build and verify these manually against `config/default.conf`:

1. `GET /` renders `Localhost Test Home`.
2. `GET /old` redirects to `/`.
3. `GET /listing/` shows directory entries.
4. `GET /cgi/echo.py?source=browser` renders the CGI page.
5. `GET /session` returns a `Set-Cookie` header on the first request and increments `X-Session-Visits` on refresh.
6. Upload a file to `/uploads` and confirm it can be fetched and deleted.

Recommended browser-devtools checks:

- Requests use `HTTP/1.1`.
- Persistent connections stay open unless `Connection: close` is sent.
- Redirect responses include `Location`.
- Error pages return the expected status codes.

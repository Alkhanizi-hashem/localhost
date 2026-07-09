# Audit Checklist

Use this as a practical audit sheet for this project. Status values mean:

- `PASS`: implemented and already covered by source code and/or automated tests.
- `MANUAL`: implemented, but should still be demonstrated live in browser/curl/devtools.
- `RECHECK`: implemented, but verification was flaky and should be repeated before the audit.
- `NO`: not implemented in this repository.

## Functional

| Audit point | What to show during audit | Project evidence | Status |
| --- | --- | --- | --- |
| How does an HTTP server work? | Explain: bind listener sockets, wait for readiness, accept clients, parse HTTP request, build response, write response, keep alive or close. | `src/main.rs`, `src/server.rs`, `src/http.rs`, `src/router.rs` | PASS |
| Which I/O multiplexing function is used and how does it work? | Say this server uses Linux `epoll`, not `select`. Show one epoll instance waiting for ready FDs. | `src/epoll.rs:13-35`, `src/server.rs:91-124`, `README.md:83` | PASS |
| Is the server using only one select/equivalent to read requests and write answers? | Show the main server loop uses one `Epoll` for listeners and client sockets. Mention CGI has its own separate epoll for CGI pipe I/O. | `src/server.rs:65-68`, `src/server.rs:100-124`, `src/cgi.rs:89-116` | PASS |
| Why is it important to use only one select and how was it achieved? | Explain one event loop avoids blocking per client and scales to many sockets in one thread. Show all listeners and clients are registered in the same epoll set. | `src/server.rs:127-203`, `src/server.rs:228-252`, `README.md:83` | PASS |
| Is there only one read or write per client per select/equivalent? | Show `read_client()` does one socket `read()` and `write_client()` does one socket `write()` for each event dispatch. | `src/server.rs:286-312`, `src/server.rs:403-459` | PASS |
| Are return values for I/O functions checked properly? | Walk through `accept`, `read`, `write`, `epoll_ctl`, `epoll_wait`; show `WouldBlock` and `Interrupted` are handled separately. | `src/server.rs:212-224`, `src/server.rs:296-312`, `src/server.rs:421-459`, `src/epoll.rs:19-35`, `src/cgi.rs:129-158` | PASS |
| If an error is returned on a socket, is the client removed? | Show `close_client(fd)` on read/write/hup/epoll-modify errors. | `src/server.rs:255-284`, `src/server.rs:308-310`, `src/server.rs:397-400`, `src/server.rs:444-447`, `src/server.rs:455-457`, `src/server.rs:564-567` | PASS |
| Is writing and reading always done through a select/equivalent? | For network sockets: yes, via epoll. For CGI subprocess pipes: also yes, via a dedicated epoll inside CGI runner. | `src/server.rs:100-124`, `src/cgi.rs:89-176` | PASS |

## Configuration File

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| Single server with single port | Run `cargo run -- config/single-port.conf`, then `curl http://127.0.0.1:8080/`. | `config/single-port.conf` | PASS |
| Multiple servers with different ports | Run `cargo run -- config/multi-port.conf`, test `:8080` and `:8081`. | `config/multi-port.conf`, `tests/integration.rs:192-253` | PASS |
| Multiple servers with different hostnames on same IP:port | Use `curl --resolve a.test:PORT:127.0.0.1 http://a.test:PORT/` and `b.test`. | `config/virtual-hosts.conf`, `src/server.rs:462-481`, `tests/integration.rs:192-253` | PASS |
| Custom error pages | Hit a missing page and show custom 400/404/405/413 page body. | `config/default.conf:10-15`, `www/errors/`, `tests/integration.rs:102-107`, `333-338` | PASS |
| Client body limit | Run with `config/body-limit.conf`, POST body shorter and longer than 16 bytes. | `config/body-limit.conf`, `src/http.rs:169-183`, `src/router.rs:19-21`, `tests/integration.rs:184-189` | PASS |
| Routes are taken into account | Show `/`, `/uploads`, `/listing`, `/cgi`, `/old` behave differently. | `config/default.conf:17-47`, `src/router.rs:27-61`, `tests/integration.rs:83-188` | PASS |
| Default file when path is a directory | `GET /` and `GET /uploads/` should serve `index.html` when present. | `config/default.conf:20`, `27`, `src/router.rs:114-132` | PASS |
| Accepted methods per route | Try forbidden `DELETE /listing/alpha.txt` and allowed `DELETE /uploads/sample.bin`. | `config/default.conf:24-46`, `src/router.rs:31-35`, `76-78`, `165-191`, `tests/integration.rs:118-123`, `149-153` | PASS |

## Methods And Cookies

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| GET works properly | Test `/`, `/listing/`, `/missing`, `/old`. | `src/router.rs:80-132`, `tests/integration.rs:83-130` | PASS |
| POST works properly | Test raw upload POST and CGI POST. | `src/router.rs:134-163`, `tests/integration.rs:132-140`, `289-297`, `323-331` | PASS |
| DELETE works properly | Delete an uploaded file, verify `204`; delete forbidden target, verify `405` or `403`. | `src/router.rs:165-191`, `tests/integration.rs:149-153` | PASS |
| Wrong request does not crash server | Send malformed HTTP/1.1 without `Host`; then send valid request after it. | `src/http.rs:147-149`, `src/server.rs:329-337`, `tests/integration.rs:300-346` | PASS |
| Uploaded files can be fetched back without corruption | Upload binary bytes and compare downloaded body exactly. | `src/router.rs:292-330`, `tests/integration.rs:132-147` | PASS |
| Session and cookies system is present | Hit `/session` twice, second request reuses cookie and increments `X-Session-Visits`. | `src/server.rs:21`, `339-358`, `483-518`, `tests/integration.rs:155-182` | PASS |

## Browser Interaction

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| Browser connects with no issue | Open `http://127.0.0.1:8080/` with `config/default.conf`. | `tests/browser_check.md:3-10` | MANUAL |
| Request and response headers are correct | In devtools Network tab, inspect `HTTP/1.1`, `Content-Length`, `Content-Type`, `Location`, cookies. | `src/http.rs:372-447`, `tests/browser_check.md:12-17` | MANUAL |
| Wrong URL handled properly | Visit `/missing`; confirm custom 404 page and correct status. | `www/errors/404.html`, `tests/integration.rs:102-107` | MANUAL |
| Directory listing handled properly | Visit `/listing/`; confirm generated listing page. | `src/router.rs:252-290`, `tests/integration.rs:109-116` | MANUAL |
| Redirect handled properly | Visit `/old`; confirm `301` and `Location: /`. | `config/default.conf:44-47`, `src/router.rs:37-50`, `tests/integration.rs:125-130` | MANUAL |
| CGI works with chunked and unchunked data | Test `/cgi/echo.py` with both `Transfer-Encoding: chunked` and `Content-Length`. | `src/cgi.rs`, `src/http.rs:158-167`, `tests/integration.rs:255-298`, `300-331` | PASS |

## Port Issues

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| Multiple ports and websites work | Run a config with shared and separate ports, then hit each host/port pair. | `tests/integration.rs:192-253` | PASS |
| Same port configured multiple times in one server should be caught | Run `cargo run -- config/duplicate-port.conf`; the duplicated server block should be discarded with warning while other valid server remains usable. | `config/duplicate-port.conf`, `src/config.rs:69-81`, `tests/integration.rs:255-298` | PASS |
| Multiple servers with common ports, one bad config should not kill valid ones | Show mixed config: invalid duplicated-port server is ignored, valid server still serves requests. | `src/config.rs:54-115`, `src/server.rs:127-203`, `tests/integration.rs:255-298` | PASS |

## Siege And Stress

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| `siege -b [IP]:[PORT]` availability at least 99.5% | Run `tests/stress.sh` or `siege -b http://127.0.0.1:8080/`. Capture result before audit. | `tests/stress.sh` | MANUAL |
| No memory leak | Run `tests/leak_check.sh` and monitor with `top`/`htop` during `tests/stress.sh`. | `tests/leak_check.sh`, `tests/leaks.md` | MANUAL |
| No hanging connection | Show timeout handling and stress behavior; check slow partial requests close with `408`. | `src/server.rs:521-555`, `tests/integration.rs:348-397`, `512-558`, `tests/stress.sh`, `tests/leak_check.sh:64-75` | PASS |

## Bonus

| Audit point | What to run/show | Project evidence | Status |
| --- | --- | --- | --- |
| More than one CGI system such as Python/C++/Perl | Current default config only ships Python CGI. You would need more `cgi` mappings and scripts to claim this. | `config/default.conf:38-41`, `www/cgi/echo.py` | NO |
| Second implementation in another language | No second implementation exists in this repository. | Repository structure | NO |

## Known Recheck Before Audit

| Item | Notes | Status |
| --- | --- | --- |
| HTTP/1.1 keep-alive verification | `cargo test` passed once, but `tests/audit_smoke.sh` hit a flaky failure in `supports_http11_keep_alive_on_same_connection`. Re-run this test and manually demo two requests on one TCP connection before the audit. | RECHECK |

## Fast Demo Commands

```sh
cargo run -- config/default.conf
curl -i http://127.0.0.1:8080/
curl -i http://127.0.0.1:8080/missing
curl -i http://127.0.0.1:8080/listing/
curl -i http://127.0.0.1:8080/old
curl -i -X POST -H "Content-Type: text/plain" --data "hello" http://127.0.0.1:8080/cgi/echo.py
curl -i -X POST -H "Content-Type: text/plain" --data "01234567890123456789" http://127.0.0.1:8080/uploads
curl -i --resolve alpha.test:8080:127.0.0.1 http://alpha.test:8080/
curl -i --resolve beta.test:8080:127.0.0.1 http://beta.test:8080/
tests/stress.sh
```

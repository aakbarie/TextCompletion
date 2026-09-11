# Scriblet Enterprise SQL Server Sync

Scriblet can synchronize an enterprise phrase library from SQL Server on Windows using ODBC and
Windows Integrated Authentication. SQL Server is never queried during text expansion. Approved
content is synchronized into the local SQLite database, and the in-memory binding index keeps
serving expansions locally.

## Enabling sync

Sync is off until both of these environment variables are set for the user or machine:

```text
SCRIBLET_SQL_SERVER=<server host>
SCRIBLET_SQL_DATABASE=<database name>
```

Scriblet ships no built-in server or database names. Set the variables through Group Policy,
Intune, a login script, or per-user environment settings.

Optional settings and their defaults:

```text
SCRIBLET_ENTERPRISE_ENABLED=true
SCRIBLET_SQL_PORT=1433
SCRIBLET_ODBC_DRIVER=ODBC Driver 18 for SQL Server
SCRIBLET_SQL_ENCRYPT=true
SCRIBLET_SQL_TRUST_SERVER_CERTIFICATE=false
SCRIBLET_SQL_LOGIN_TIMEOUT_SECONDS=5
```

The generated connection string is:

```text
Driver={ODBC Driver 18 for SQL Server};Server=<host>,<port>;Database=<database>;Trusted_Connection=Yes;Encrypt=Yes;TrustServerCertificate=No;
```

Scriblet does not store SQL usernames or passwords. Set `SCRIBLET_ENTERPRISE_ENABLED=false` to
disable synchronization while keeping the personal library.

## Client prerequisites

- Windows machine able to authenticate to SQL Server with the logged-in Windows identity
- Microsoft ODBC Driver 18 for SQL Server installed (or another driver named in `SCRIBLET_ODBC_DRIVER`)
- Network path to the server and port
- SELECT permission on `dbo.ScribletSnippets` and `dbo.ScribletBindings`

Driver 18 requires a trusted certificate when `Encrypt=Yes`. For a server with a self-signed
certificate, either install the certificate on clients or set
`SCRIBLET_SQL_TRUST_SERVER_CERTIFICATE=true`.

## Sync behaviour

- Sync runs on a background thread when Scriblet starts and again when the user clicks
  **Sync enterprise library**. The window is usable while it runs.
- The whole sync is one SQLite transaction. If the server is unreachable or a row is invalid,
  the previously cached library stays intact.
- Enterprise snippets that disappear from the server are removed from the cache.
- Personal snippets are never modified. If an enterprise binding uses the same text as a
  personal binding, the personal binding wins, the enterprise binding is skipped, and the skip
  is listed in the status badge and the log.
- Enterprise snippets are read-only in the editor. Users can duplicate them into personal copies.
- The login timeout keeps an off-network laptop from waiting long; the status badge reports
  "Offline · using cached enterprise library" with the reason.

## Server schema

Run `docs/sqlserver-schema.sql` in the target database. Normal Scriblet clients should receive
SELECT permission only. Publish content through a separate administrative process or a
controlled database role.

Only `kind = 'text'` bindings are supported. The column is reserved for future binding kinds.

## Privacy boundary

The synchronization query reads only centrally managed snippet content and bindings. Scriblet
does not send typed keystrokes, clipboard history, expansion events, telemetry, or PHI to SQL
Server. The local log records sync results and hook status, never typed text.

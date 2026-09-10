# Scriblet Enterprise SQL Server Sync

Scriblet v0.4 can synchronize an enterprise phrase library directly from SQL Server on Windows using ODBC and Windows Integrated Authentication. SQL Server is never queried during text expansion. Approved content is synchronized into the existing local SQLite database and the in-memory binding index continues to serve expansions locally.

## Alliance deployment defaults

The current Alliance reporting environment is configured as:

```text
Driver={SQL Server}
Server=RPTPRODDB
Port=1433
Database=Alliance_RPT
Trusted_Connection=Yes
Encrypt=Yes
TrustServerCertificate=No
```

Scriblet uses these as Windows defaults in v0.4. Environment variables can override them for other environments.

## Client prerequisites

- Windows machine joined to the appropriate domain or otherwise able to authenticate to SQL Server with the logged-in Windows identity
- `SQL Server` ODBC driver installed
- Network path to `RPTPRODDB:1433`
- SELECT permission on `dbo.ScribletSnippets` and `dbo.ScribletBindings`

## Deployment configuration

Optional overrides:

```text
SCRIBLET_ENTERPRISE_ENABLED=true
SCRIBLET_SQL_SERVER=RPTPRODDB
SCRIBLET_SQL_PORT=1433
SCRIBLET_SQL_DATABASE=Alliance_RPT
SCRIBLET_ODBC_DRIVER=SQL Server
SCRIBLET_SQL_ENCRYPT=true
SCRIBLET_SQL_TRUST_SERVER_CERTIFICATE=false
```

The generated connection string is equivalent to:

```text
Driver={SQL Server};Server=RPTPRODDB,1433;Database=Alliance_RPT;Trusted_Connection=Yes;Encrypt=Yes;TrustServerCertificate=No;
```

Scriblet does not store SQL usernames or passwords. Set `SCRIBLET_ENTERPRISE_ENABLED=false` to disable enterprise synchronization while retaining the local personal library.

## Server schema

Run `docs/sqlserver-schema.sql` in `Alliance_RPT`. Normal Scriblet clients should receive SELECT permission only. Content publishing should be performed through a separate administrative process or controlled database role.

## Offline behavior

At startup Scriblet attempts an enterprise synchronization. If SQL Server cannot be reached, the last synchronized enterprise library remains in SQLite and continues to work. Personal snippets are not removed or overwritten during enterprise synchronization.

## Privacy boundary

The synchronization query reads only centrally managed snippet content and bindings. Scriblet does not send typed keystrokes, clipboard history, expansion events, telemetry, or PHI to SQL Server.

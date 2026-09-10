# Scriblet Enterprise SQL Server Sync

Scriblet v0.4 can synchronize an enterprise phrase library directly from SQL Server on Windows using ODBC and Windows Integrated Authentication. SQL Server is never queried during text expansion. Approved content is synchronized into the existing local SQLite database and the in-memory binding index continues to serve expansions locally.

## Client prerequisites

- Windows machine joined to the appropriate domain or otherwise able to authenticate to SQL Server with the logged-in Windows identity
- Microsoft ODBC Driver 18 for SQL Server installed
- Network path to the SQL Server instance
- SELECT permission on `dbo.ScribletSnippets` and `dbo.ScribletBindings`

## Deployment configuration

Set these environment variables for the user or machine running Scriblet:

```text
SCRIBLET_SQL_SERVER=SQLPROD01
SCRIBLET_SQL_DATABASE=Scriblet
SCRIBLET_ODBC_DRIVER=ODBC Driver 18 for SQL Server
SCRIBLET_SQL_ENCRYPT=true
SCRIBLET_SQL_TRUST_SERVER_CERTIFICATE=false
```

Only `SCRIBLET_SQL_SERVER` and `SCRIBLET_SQL_DATABASE` are required. The remaining values default to the secure settings shown above.

The generated connection string uses:

```text
Trusted_Connection=Yes;Encrypt=Yes;TrustServerCertificate=No;
```

Scriblet does not store SQL usernames or passwords.

## Server schema

Run `docs/sqlserver-schema.sql` in the target database. Normal Scriblet clients should receive SELECT permission only. Content publishing should be performed through a separate administrative process or controlled database role.

## Offline behavior

At startup Scriblet attempts an enterprise synchronization when SQL Server configuration is present. If SQL Server cannot be reached, the last synchronized enterprise library remains in SQLite and continues to work. Personal snippets are not removed or overwritten during enterprise synchronization.

## Privacy boundary

The synchronization query reads only centrally managed snippet content and bindings. Scriblet does not send typed keystrokes, clipboard history, expansion events, telemetry, or PHI to SQL Server.

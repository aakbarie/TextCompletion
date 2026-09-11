-- Scriblet enterprise library schema (v0.4+)
-- SQL Server is the source of truth. Clients receive read-only access to these tables.

CREATE TABLE dbo.ScribletSnippets (
    id UNIQUEIDENTIFIER NOT NULL CONSTRAINT PK_ScribletSnippets PRIMARY KEY,
    title NVARCHAR(200) NOT NULL,
    category NVARCHAR(100) NOT NULL,
    replacement NVARCHAR(MAX) NOT NULL,
    enabled BIT NOT NULL CONSTRAINT DF_ScribletSnippets_enabled DEFAULT (1),
    version BIGINT NOT NULL CONSTRAINT DF_ScribletSnippets_version DEFAULT (1),
    updated_at BIGINT NOT NULL
);
GO

CREATE TABLE dbo.ScribletBindings (
    id UNIQUEIDENTIFIER NOT NULL CONSTRAINT PK_ScribletBindings PRIMARY KEY,
    snippet_id UNIQUEIDENTIFIER NOT NULL,
    -- Only 'text' is supported by clients today; the column is reserved for future kinds.
    kind NVARCHAR(20) NOT NULL CONSTRAINT DF_ScribletBindings_kind DEFAULT (N'text'),
    value NVARCHAR(255) NOT NULL,
    enabled BIT NOT NULL CONSTRAINT DF_ScribletBindings_enabled DEFAULT (1),
    CONSTRAINT FK_ScribletBindings_Snippet FOREIGN KEY (snippet_id)
        REFERENCES dbo.ScribletSnippets(id) ON DELETE CASCADE,
    CONSTRAINT UQ_ScribletBindings_kind_value UNIQUE (kind, value)
);
GO

CREATE INDEX IX_ScribletSnippets_category
    ON dbo.ScribletSnippets(category, enabled);
GO

CREATE INDEX IX_ScribletBindings_snippet
    ON dbo.ScribletBindings(snippet_id, enabled);
GO

-- Recommended authorization pattern. Replace DOMAIN and group names for the deployment.
-- CREATE LOGIN [DOMAIN\ScribletUsers] FROM WINDOWS;
-- CREATE USER [DOMAIN\ScribletUsers] FOR LOGIN [DOMAIN\ScribletUsers];
-- GRANT SELECT ON dbo.ScribletSnippets TO [DOMAIN\ScribletUsers];
-- GRANT SELECT ON dbo.ScribletBindings TO [DOMAIN\ScribletUsers];
-- Do not grant INSERT, UPDATE, or DELETE to normal Scriblet clients.

{{/*
Expand the name of the chart.
*/}}
{{- define "rust-docs-mcp-server.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "rust-docs-mcp-server.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "rust-docs-mcp-server.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "rust-docs-mcp-server.labels" -}}
helm.sh/chart: {{ include "rust-docs-mcp-server.chart" . }}
{{ include "rust-docs-mcp-server.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "rust-docs-mcp-server.selectorLabels" -}}
app.kubernetes.io/name: {{ include "rust-docs-mcp-server.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "rust-docs-mcp-server.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "rust-docs-mcp-server.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Create the PostgreSQL connection string
*/}}
{{- define "rust-docs-mcp-server.databaseUrl" -}}
{{- if .Values.postgresql.enabled }}
{{- printf "postgresql://%s:%s@%s-postgresql:5432/%s" .Values.postgresql.auth.username .Values.postgresql.auth.password .Release.Name .Values.postgresql.auth.database }}
{{- else }}
{{- printf "postgresql://%s:%s@%s:%d/%s" .Values.externalDatabase.username .Values.externalDatabase.password .Values.externalDatabase.host .Values.externalDatabase.port .Values.externalDatabase.database }}
{{- end }}
{{- end }}

{{/*
Create the secret name for API keys
*/}}
{{- define "rust-docs-mcp-server.secretName" -}}
{{- if .Values.app.existingSecret }}
{{- .Values.app.existingSecret }}
{{- else }}
{{- include "rust-docs-mcp-server.fullname" . }}-secrets
{{- end }}
{{- end }}

{{/*
Create the config map name
*/}}
{{- define "rust-docs-mcp-server.configMapName" -}}
{{- include "rust-docs-mcp-server.fullname" . }}-config
{{- end }}

{{/*
Create the image name with tag
*/}}
{{- define "rust-docs-mcp-server.image" -}}
{{- $tag := .Values.image.tag | default .Chart.AppVersion }}
{{- printf "%s:%s" .Values.image.repository $tag }}
{{- end }}
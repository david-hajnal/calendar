{{- define "commoncal-auth.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "commoncal-auth.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := include "commoncal-auth.name" . }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "commoncal-auth.baseLabels" -}}
helm.sh/chart: {{ include "commoncal-auth.name" . }}-{{ .Chart.Version | replace "+" "_" }}
app.kubernetes.io/name: {{ include "commoncal-auth.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "commoncal-auth.labels" -}}
{{ include "commoncal-auth.baseLabels" . }}
app.kubernetes.io/component: authorization
{{- end }}

{{- define "commoncal-auth.selectorBaseLabels" -}}
app.kubernetes.io/name: {{ include "commoncal-auth.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "commoncal-auth.selectorLabels" -}}
{{ include "commoncal-auth.selectorBaseLabels" . }}
app.kubernetes.io/component: authorization
{{- end }}

{{- define "commoncal-auth.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "commoncal-auth.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- required "serviceAccount.name is required when serviceAccount.create is false" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "commoncal-auth.publicServiceName" -}}
{{- printf "%s-public" (include "commoncal-auth.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "commoncal-auth.internalServiceName" -}}
{{- printf "%s-internal" (include "commoncal-auth.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

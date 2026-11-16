{{/* The name every object of the release starts with. */}}
{{- define "codehub.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 50 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 50 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{/* The labels of every object. */}}
{{- define "codehub.labels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{/* The labels that select the pods of one component (a role, the registry, the runner). Takes "root" and "component". */}}
{{- define "codehub.selector" -}}
app.kubernetes.io/name: {{ .root.Chart.Name }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{/* The server image. */}}
{{- define "codehub.image" -}}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) -}}
{{- end -}}

{{/* A Secret's value as an environment variable. Takes "name", "secret" (a {name, key} value) and "what" for the message. */}}
{{- define "codehub.secretEnv" -}}
- name: {{ .name }}
  valueFrom:
    secretKeyRef:
      name: {{ required (printf "%s names no Secret" .what) .secret.name }}
      key: {{ required (printf "%s names no key" .what) .secret.key }}
{{- end -}}

{{/* The pod anti-affinity of a component's replicas. Takes "root", "component" and "mode". */}}
{{- define "codehub.antiAffinity" -}}
{{- if or (eq .mode "soft") (eq .mode "hard") -}}
affinity:
  podAntiAffinity:
{{- if eq .mode "soft" }}
    preferredDuringSchedulingIgnoredDuringExecution:
      - weight: 100
        podAffinityTerm:
          topologyKey: kubernetes.io/hostname
          labelSelector:
            matchLabels:
              {{- include "codehub.selector" . | nindent 14 }}
{{- else }}
    requiredDuringSchedulingIgnoredDuringExecution:
      - topologyKey: kubernetes.io/hostname
        labelSelector:
          matchLabels:
            {{- include "codehub.selector" . | nindent 12 }}
{{- end }}
{{- else if ne .mode "" -}}
{{- fail (printf "%s.antiAffinity is %q, not soft, hard or empty" .component .mode) -}}
{{- end -}}
{{- end -}}

{{/* A HorizontalPodAutoscaler on CPU. Takes "root", "component", "target" (the Deployment's name) and "values". */}}
{{- define "codehub.hpa" -}}
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: {{ .target }}
  labels:
    {{- include "codehub.labels" .root | nindent 4 }}
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: {{ .target }}
  minReplicas: {{ .values.autoscaling.minReplicas }}
  maxReplicas: {{ .values.autoscaling.maxReplicas }}
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: {{ .values.autoscaling.targetCPUUtilizationPercentage }}
{{- end -}}

{{/* A PodDisruptionBudget. Takes "root", "component", "target" and "values". */}}
{{- define "codehub.pdb" -}}
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: {{ .target }}
  labels:
    {{- include "codehub.labels" .root | nindent 4 }}
spec:
{{- if not (kindIs "invalid" .values.podDisruptionBudget.minAvailable) }}
  minAvailable: {{ .values.podDisruptionBudget.minAvailable }}
{{- else }}
  maxUnavailable: {{ required (printf "%s.podDisruptionBudget sets neither minAvailable nor maxUnavailable" .component) .values.podDisruptionBudget.maxUnavailable }}
{{- end }}
  selector:
    matchLabels:
      {{- include "codehub.selector" . | nindent 6 }}
{{- end -}}

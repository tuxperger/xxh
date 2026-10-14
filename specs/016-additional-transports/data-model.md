# Data Model: Транспорт Kubernetes

## KubeTarget (`xxh-transport`)

| Поле | Тип | Смысл |
| --- | --- | --- |
| `pod` | `String` | имя пода |
| `namespace` | `Option<String>` | `None` — пространство текущего контекста |
| `container` | `Option<String>` | `None` — контейнер по умолчанию (R3) |
| `context` | `Option<String>` | `--kube-context`; `None` — текущий контекст |
| `label` | `String` | адрес как написал пользователь, без `k8s:` (`ns/pod:c`) — для прогресса и `status` |
| `connect_timeout_s` | `u64` | общий таймаут подключения |

`ResolvedTarget::Kube(KubeTarget)`; `label()` и `connect_timeout_s()` покрывают
новый вариант.

## ParsedTarget::Kube (`xxh-cli::target`)

`{ namespace: Option<String>, pod: String, container: Option<String> }` — результат
чистого разбора. Ошибки: пустой под, пустое пространство имён (`k8s:/pod`), пустой
контейнер (`k8s:pod:`), лишние разделители (`k8s:a/b/c`, `k8s:p:c:d`).

## PodInfo (внутренний, `kube_backend`)

`{ phase: String, default_container: Option<String>, containers: Vec<String> }` —
разбор строки `phase|annotation|c1 c2 …`.

## CliTargetFlags

Новое: `user_set` (`-l/--user`, недопустим для k8s), `kube_context_set`
(допустим только для k8s).

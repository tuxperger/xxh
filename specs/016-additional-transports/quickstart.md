# Quickstart: Kubernetes

```bash
# под в пространстве имён текущего контекста
xxh k8s:web-7d9f

# пространство имён и контейнер
xxh k8s:payments/api-0:app

# другой контекст kubeconfig
xxh --kube-context staging k8s:payments/api-0

# одна команда, код возврата — её
xxh k8s:payments/api-0 -- ls -la /srv

# сохранить окружение, посмотреть, удалить
xxh --keep k8s:payments/api-0
xxh status k8s:payments/api-0
xxh clean k8s:payments/api-0
```

Проверка вручную (kind):

```bash
kind create cluster --name xxh
kubectl run xxh-t --image=alpine:3.20 -- sleep infinity
kubectl wait --for=condition=Ready pod/xxh-t
xxh k8s:xxh-t -- echo ok
kubectl exec xxh-t -- sh -c 'test -e ~/.xxh && echo DIRTY || echo CLEAN'
kind delete cluster --name xxh
```

Интеграция: `nix develop -c cargo test -p xxh-cli --test k8s_smoke` (кластер из
текущего контекста или `XXH_TEST_KUBE_CONTEXT`; без кластера тест пропускается).

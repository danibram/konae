# Verificación del 21 de octubre de 2026

Esta pasada comprobó el flujo del editor, la publicación, la persistencia de datos y la exportación a un proyecto Rust autoalojado. No se configuró un proveedor ni se ejecutó un LLM, según lo pedido.

Las comprobaciones automatizadas terminaron con 219 pruebas Bun y 9 pruebas Rust aprobadas. En Chrome pasaron cuatro recorridos de extremo a extremo que cubren la compilación con imports múltiples, la persistencia tras recargar, la republicación con la misma URL y base de datos, y las copias de seguridad, restauración, exportación, autenticación Alice/Bob y migraciones local/desplegada.

También se probó el flujo con una app creada desde la interfaz con agent-browser conectado a Helium mediante CDP. El proyecto `Notas de Helium` tenía el identificador `app_94dafa63-8d59-46ad-8ece-2ff8267ceba3`. Se editó en `http://127.0.0.1:4175` y se publicó en `http://notas-de-helium-c0616d.localhost:8792`. Tras crear dos cuentas, la primera guardó una nota que sobrevivió a una recarga y a cerrar sesión; la segunda no vio esa nota.

Desde la interfaz se descargó el archivo `/tmp/helium-selfhost.tar`. Se compiló el proyecto Rust exportado con la configuración de despliegue generada y se ejecutó en el puerto 8793, en `http://notas-de-helium-56c870.localhost:8793`. La importación recuperó una nota y dos usuarios, sin sesiones activas. La cuenta original pudo iniciar sesión y ver su nota. Una nota nueva también sobrevivió a una recarga en el proyecto autoalojado. La captura de esa prueba está en [helium-selfhost.png](./verification-assets/helium-selfhost.png). La descarga final actualizada quedó en `/tmp/helium-selfhost-final.tar`.

La copia autoalojada guardó una segunda nota mientras la app original conservó solo la primera. Las pruebas Rust también comprobaron que republicar permite crear tablas e índices sin perder filas y que un esquema destructivo o incompatible con las acciones provoca una reversión.

La app local continúa en `data/kernel-local`. Para reiniciar el editor y el Kernel en los puertos usados durante la prueba:

```bash
LITTLEBOX_EDITOR_PORT=4175 LITTLEBOX_KERNEL_PORT=8792 bun run dev:local
```

La verificación corresponde al prototipo local. El acceso de administración usa la clave de desarrollo local, el registro de cuentas está abierto y no existe un sistema de invitaciones. Los archivos tienen un límite de 2 MiB y el estado de la app, de 16 MiB. Esta pasada no valida el uso de un proveedor LLM.

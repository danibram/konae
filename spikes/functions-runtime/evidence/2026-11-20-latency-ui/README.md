# Panel de latencias

Pasan las 12 pruebas de [tests.txt](tests.txt), con 41 aserciones. Las tres pruebas del componente comprueban percentiles y límite, resultados inciertos, reintentos y ausencia del panel cuando no hay muestras. Las otras nueve prueban los cálculos existentes de Usage.

La revisión en Chrome usa un perfil temporal y datos sintéticos del componente. [browser.json](browser.json) registra que el desglose se abre, que no hay errores JavaScript y que la página no desborda los 390 píxeles del móvil. Las capturas de [escritorio](desktop.png) y [móvil](mobile.png) muestran el componente con las hojas de estilo del producto. Esta prueba no necesita credenciales ni accede a una app del usuario.

[manifest.json](manifest.json) identifica el comando y los archivos capturados en [tested-source.tar.gz](tested-source.tar.gz). Los valores de las capturas son ejemplos de prueba, no mediciones del runtime. Esta evidencia no demuestra que el dispatcher registre llamadas reales; esa comprobación corresponde a las pruebas de integración del Kernel.

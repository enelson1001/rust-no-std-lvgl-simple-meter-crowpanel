
 ### The mock_include Directory:
 During the build process, the `bindgen` search path is directed to a `mock_include` 
 directory. This directory contains "hollow" versions of standard headers like 
 <stdio.h>, <string.h>, and <assert.h>. 

 These mock files are essential because:
 - **Dependency Satisfaction:** LVGL's internal headers include these standard 
 files via `#include <...>` directives.
 - **Avoiding Host Pollution:** Instead of using the host machine's headers (which 
 may contain architecture-specific C++ or OS features), the mock headers provide 
 just enough surface area to allow the parser to proceed without errors.



### Mock File,Purpose
- stdint.h  Empty, Types are already manually defined in wrapper.h

- string.h  Empty, Prevents the compiler from trying to pull in host-OS optimized string functions.

- stdio.h  Empty, LVGL has its own internal string formatting logic; we don't want the no_std build to depend on a non-existent system vsnprintf.

- stdbool.h Empty, Types are already manually defined in wrapper.h

- stdarg.h  Empty, Because wrapper.h already handles the va_list definition for bindgen


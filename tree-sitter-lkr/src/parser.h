#include <tree_sitter/parser.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifdef _WIN32
#define DLL_PUBLIC __declspec(dllexport)
#else
#define DLL_PUBLIC __attribute__ ((visibility ("default")))
#endif

DLL_PUBLIC const TSLanguage *tree_sitter_lkr(void);

#ifdef __cplusplus
}
#endif
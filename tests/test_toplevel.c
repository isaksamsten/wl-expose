#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "toplevel.h"

static int failures;

#define CHECK(cond, msg) \
	do { \
		if (!(cond)) { \
			fprintf(stderr, "%s:%d: check failed: %s: %s\n", __FILE__, __LINE__, #cond, msg); \
			failures++; \
		} \
	} while (0)

static struct toplevel tls[5];

static void setup(struct wl_list *list) {
	static const char *ids[] = {"10", "2", "abc", "100", "x y"};
	wl_list_init(list);
	for (size_t i = 0; i < 5; i++) {
		tls[i] = (struct toplevel){
			.identifier = (char *)ids[i],
			.app_id = (char *)"app",
			.title = (char *)"title",
			.done = true,
		};
		wl_list_insert(list->prev, &tls[i].link);
	}
}

int main(void) {
	struct wl_list list;
	struct toplevel *out[5];
	setup(&list);

	// No IDs: all windows, announcement order.
	size_t n = toplevel_select(&list, NULL, 0, out, NULL);
	CHECK(n == 5 && out[0] == &tls[0] && out[4] == &tls[4], "all windows");

	// IDs: exact matches only, in ID order.
	char *ids1[] = {"100", "10", "1", "x"};
	n = toplevel_select(&list, ids1, 4, out, NULL);
	CHECK(n == 2 && out[0] == &tls[3] && out[1] == &tls[0], "exact match, id order");

	// Duplicates show once; spaces are part of the ID.
	char *ids2[] = {"2", "x y", "2"};
	n = toplevel_select(&list, ids2, 3, out, NULL);
	CHECK(n == 2 && out[0] == &tls[1] && out[1] == &tls[4], "duplicates");

	// Unknown IDs are warned about and skipped.
	char *ids3[] = {"nope", "abc"};
	FILE *warn = tmpfile();
	n = toplevel_select(&list, ids3, 2, out, warn);
	CHECK(n == 1 && out[0] == &tls[2], "unknown skipped");
	char buf[256] = {0};
	rewind(warn);
	size_t len = fread(buf, 1, sizeof(buf) - 1, warn);
	(void)len;
	fclose(warn);
	CHECK(strstr(buf, "'nope'") != NULL, "warning printed");
	CHECK(strstr(buf, "abc") == NULL, "no warning for found ID");

	// Nothing matches: empty result.
	char *ids4[] = {"zzz"};
	n = toplevel_select(&list, ids4, 1, out, NULL);
	CHECK(n == 0, "no match");

	// Closed windows are never selected; NULL identifiers never match.
	tls[0].closed = true;
	tls[1].identifier = NULL;
	char *ids5[] = {"10", "2", "abc"};
	n = toplevel_select(&list, ids5, 3, out, NULL);
	CHECK(n == 1 && out[0] == &tls[2], "closed / null identifier");
	n = toplevel_select(&list, NULL, 0, out, NULL);
	CHECK(n == 4 && out[0] == &tls[1], "all skips closed");

	// --list output format, with tabs/newlines flattened.
	tls[2].title = (char *)"a\tb\nc";
	FILE *lst = tmpfile();
	toplevel_print_list(&list, lst);
	rewind(lst);
	memset(buf, 0, sizeof(buf));
	len = fread(buf, 1, sizeof(buf) - 1, lst);
	fclose(lst);
	CHECK(strstr(buf, "abc\tapp\ta b c\n") != NULL, "list format");
	CHECK(strstr(buf, "10\t") == NULL, "list skips closed");

	if (failures) {
		fprintf(stderr, "%d failure(s)\n", failures);
		return 1;
	}
	printf("ok\n");
	return 0;
}


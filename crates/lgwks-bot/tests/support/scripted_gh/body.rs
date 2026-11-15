//! The scripted `gh` client's shell text, kept apart from the fixture that
//! installs it so each file stays inside the size law.

/// The scripted client itself, without its `#!` line (see `interpreter`).
///
/// POSIX `sh`, no `jq`, no `python`: the estate's portability rules apply to a
/// fixture too, and a script that needs a JSON processor to answer a JSON
/// client is a fixture with its own dependency. The behaviour file is cut with
/// parameter expansion, and the reviews list is emitted verbatim from the file
/// so a scenario controls exactly what the read-back returns.
///
/// The argv record is written **before** the behaviour is applied, so a create
/// that dies mid-way is still recorded. That is the point: the question the
/// tests ask is how many times the code under test *issued* a create, which is
/// what this log counts.
pub const SCRIPT_BODY: &str = r#"# A scripted `gh api` for the adapter's tests. Records argv, then answers from a
# behaviour file. Never touches the network.
set -u

# Every fork below is paid once per call, and a saturation family makes tens of
# thousands of calls, so the bookkeeping avoids command substitution wherever a
# parameter expansion answers the same question. `$0` is the path `execvp`
# resolved, so stripping its last component is what `dirname` printed.
case "$0" in
  */*) dir=${0%/*} ;;
  *) dir=. ;;
esac
log="$dir/argv.log"
behaviour="$dir/behaviour.json"

# Record argv first: a call that fails afterwards still happened, and counting
# the calls that *started* is the measurement the duplicate-post tests need.
# The whole line is assembled in memory and written with one append, so two
# concurrent runs of the script cannot interleave halves of a line.
# A literal tab, kept out of a command substitution: a saturation family makes
# tens of thousands of calls and every `$(...)` is a fork of its own.
tab='	'
line=""
for arg in "$@"; do
  line="$line$arg$tab"
done
printf '%s\n' "$line" >> "$log"

# The behaviour file is one line of JSON the fixture itself writes, so a field
# is read once per invocation and cut with parameter expansion rather than by
# forking `sed` per lookup. A saturation family runs five calls per review
# (snapshot, diff, freshness, publish, verify), and the fixture's own
# bookkeeping must not dominate the measurement, so the read is one builtin and
# each lookup is one expansion.
#
# Read once rather than per lookup: a key present under neither the quoted nor
# the numeric shape yields the empty string, which is exactly what the callers
# treat as "unspecified". The value charset is the fixture's own — alphanumeric
# plus `:._-` — so a value ends at the first character outside it.
BEHAVIOUR=""
IFS= read -r BEHAVIOUR < "$behaviour"

field() {
  val=""
  q="\"$1\":\""
  rest=${BEHAVIOUR#*"$q"}
  if [ "$rest" = "$BEHAVIOUR" ]; then
    u="\"$1\":"
    rest=${BEHAVIOUR#*"$u"}
    if [ "$rest" = "$BEHAVIOUR" ]; then
      return 0
    fi
  fi
  val=${rest%%[!a-zA-Z0-9:._-]*}
}

# The staged payload, when this call has one.
payload=""
previous=""
for arg in "$@"; do
  if [ "$previous" = "--input" ] && [ -f "$arg" ]; then
    # The staged payload is one line of JSON the adapter wrote, so it is read
    # with the builtin rather than forked through `cat`.
    IFS= read -r payload < "$arg"
    # The adapter removes the staged file as soon as the child exits, so the
    # payload is copied into the receiver's own store here: what the receiver
    # actually received, which is what a test asserts against.
    printf '%s\n' "$payload" >> "$dir/received.jsonl"
  fi
  previous="$arg"
done

method="GET"
previous=""
for arg in "$@"; do
  if [ "$previous" = "--method" ]; then method="$arg"; fi
  previous="$arg"
done

# Optional flooding, for the bounded-capture test: emit `flood_bytes` of
# padding on stdout before anything else, so the adapter's ceiling is what
# decides what it retains.
field flood_bytes; flood=$val
if [ -n "${flood:-}" ] && [ "$flood" -gt 0 ] 2>/dev/null; then
  i=0
  while [ "$i" -lt "$flood" ]; do
    printf 'PADDINGPADDINGPADDINGPADDINGPADDINGPADDINGPADDINGPADDING\n'
    i=$((i + 1))
  done
fi

# An optional hang, for the deadline test.
field hang_seconds; hang=$val
if [ -n "${hang:-}" ] && [ "$hang" -gt 0 ] 2>/dev/null; then
  # A grandchild, so the deadline has a process *group* to reap rather than
  # one child it could trivially kill.
  ( sleep "$hang" ) &
  sleep "$hang"
  exit 0
fi

if [ "$method" = "POST" ]; then
  field create; create=$val
  case "$create" in
    accept|accept_then_drop|pending|partial)
      # A real receiver assigns a fresh id per accepted create. The append is
      # one atomic write, so the count of accepted creates is exact; the count
      # is read back with the builtin (`read` takes the append log whole and
      # `${#..}` is its byte count) rather than by forking `wc`. The read
      # follows the append, so two creates racing here can read the same count
      # and share an id — the same window this counter always had — and that is
      # safe: a read-back reconciles on subject, body and state, and every
      # id-asserting family runs its creates sequentially.
      printf 'x' >> "$dir/creates"
      seen_raw=""
      IFS= read -r seen_raw < "$dir/creates"
      seen=${#seen_raw}
      field next_review_id
      id=$(( val + seen - 1 ))
      # GitHub reports the state a review is *in*, not the event that created
      # it: `COMMENT` reads back as `COMMENTED`, and so on. A script that echoed
      # the event would let a verifier pass here that never matches GitHub.
      field created_state; state=$val
      event=${payload##*\"event\":\"}
      event=${event%%\"*}
      case "$event" in
        COMMENT) state=COMMENTED ;;
        APPROVE) state=APPROVED ;;
        REQUEST_CHANGES) state=CHANGES_REQUESTED ;;
      esac
      # A pending draft is never submitted, whatever event the request named:
      # it is the state GitHub holds a create in when no event is submitted, and
      # the read-back must report it as a draft rather than as a publication.
      if [ "$create" = "pending" ]; then state=PENDING; fi
      # The record is rendered from the *payload the adapter sent*, not from the
      # scenario's defaults. That is what makes the read-back a real
      # observation: a receiver that answered from its own configuration would
      # verify a body the adapter never published, and the lost-response test
      # would pass without the write having produced anything.
      #
      # An inline comment also carries a `body`, and the match is greedy, so the
      # comments array is removed before the top-level body is read: otherwise a
      # payload with comments would record the last comment's text as the review
      # body.
      top=${payload%%,\"comments\"*}
      body=${top##*\"body\":\"}
      body=${body%%\"*}
      commit=${payload##*\"commit_id\":\"}
      commit=${commit%%\"*}
      # How many inline comments this review reports: none for everything but a
      # partial landing, which reports the configured applied count — fewer than
      # the payload intended, which is what makes it a *partial* submission.
      comments=0
      if [ "$create" = "partial" ]; then field applied_comments; comments=$val; fi
      printf '%s\n' "$payload" > "$dir/applied-$id.json"
      # The separator goes into `reviews.jsonl` as **one** `>>` append, always, in
      # front of the record. `>>` opens `O_APPEND`, and two writes by
      # descriptors on a regular file cannot interleave, so one append is atomic
      # against every other append however many creates race.
      #
      # Two earlier spellings were not safe, and both produced a read-back a
      # concurrent receiver could not answer: writing the separator separately,
      # which the kernel interleaved into `[,,{...}]`, and probing for the file
      # to decide whether a separator was owed, which two creates could both
      # read as absent into `{...}{...}`. There is no first record to make bare
      # here, so there is nothing to probe.
      #
      # The record ends with a newline, and that newline is its commit mark. An
      # append is atomic against other *appends*, not against a *reader*: on
      # tmpfs (CI's `TMPDIR=/dev/shm`) the copy goes a page at a time and the
      # file grows as it goes, so a concurrent read-back can see the front half
      # of another run's record. CI saw exactly that, a read-back cut mid-string
      # at byte 8,193 (two pages). A JSON-encoded record never holds a raw
      # newline, so a line without one is a record still being written, and the
      # reader below leaves it out.
      printf ',{"id":%s,"commit_id":"%s","state":"%s","body":"%s","comment_count":%s}\n' \
        "$id" "$commit" "$state" "$body" "${comments:-0}" >> "$dir/reviews.jsonl"
      if [ "$create" = "accept" ]; then
        printf '{"id":%s,"commit_id":"%s","state":"%s","body":"%s","comment_count":%s}\n' \
          "$id" "$commit" "$state" "$body" "${comments:-0}"
        exit 0
      fi
      # The effect lands and the *response* is lost: the review is recorded in
      # the receiver, then the connection dies before anything is printed. This
      # is the case the whole reconciliation path exists for.
      printf 'connection reset by peer\n' >&2
      exit 7
      ;;
    *)
      printf 'create failed: %s\n' "$create" >&2
      exit 1
      ;;
  esac
fi

# A read. The review list and the changed-file inventory are each identified by
# scanning argv for their path, not by position: `gh api --method GET
# .../reviews --paginate` puts a flag after the path, so a suffix match over the
# whole vector would read the pull request instead.
is_reviews=0
is_files=0
for arg in "$@"; do
  case "$arg" in
    */reviews) is_reviews=1 ;;
    */files) is_files=1 ;;
  esac
done

if [ "$is_reviews" -eq 1 ]; then
    field fail_reads; fail=$val
    if [ -n "${fail:-}" ] && [ "$fail" -gt 0 ] 2>/dev/null; then
      printf 'read refused by scenario\n' >&2
      exit 1
    fi
    # A permission refusal, for the lost-read-permission probe: the shape `gh`
    # prints when a credential cannot reach the resource, which the adapter
    # reports as its own typed permission failure.
    field deny_reads; deny=$val
    if [ "$deny" = "1" ]; then
      printf 'gh: Bad credentials (HTTP 403)\n' >&2
      exit 1
    fi
    # A malformed answer, for the decode-refusal probes. `garbage` is not JSON;
    # `truncated` is a JSON document that lost its closing bracket, which is the
    # shape a stream cut mid-write takes when everything before the cut was
    # valid. Both must be refused rather than decoded into a partial list.
    field reviews_shape; shape=$val
    case "$shape" in
      garbage)
        printf 'gh: this is not what you asked for\n'
        exit 0
        ;;
    esac
    # Records in the order the endpoint lists them, oldest first: the history
    # filler, then each review the receiver accepted. Each record is preceded
    # by what stands before it on the wire, decided by `separate`: nothing
    # before the first, a comma between two on one page, and at a page
    # boundary either the end of the answer (a read that did not ask for
    # `--paginate` gets the first page and no more, as the REST endpoint
    # answers a client that ignores the `Link` header), a comma (`gh api
    # --paginate` merging array pages into one array), or `][` for the
    # `concatenated` shape, the older client's one array per page.
    field review_page; page=$val
    paginate=0
    for arg in "$@"; do
      if [ "$arg" = "--paginate" ]; then paginate=1; fi
    done
    at=0
    ended=0
    separate() {
      if [ "$at" -eq 0 ]; then
        return 0
      fi
      if [ -n "${page:-}" ] && [ "$page" -gt 0 ] 2>/dev/null && [ $((at % page)) -eq 0 ]; then
        if [ "$paginate" -eq 0 ]; then
          ended=1
          return 1
        fi
        if [ "$shape" = "concatenated" ]; then
          printf ']['
        else
          printf ','
        fi
        return 0
      fi
      printf ','
    }
    printf '['
    # Filler reviews, for the review-ceiling and pagination probes. They are
    # real records on another commit: what matters is that the adapter would
    # have had to read them to call the list complete, so returning only the
    # prefix would be a lie about the pull request's review history.
    field filler_reviews; filler=$val
    i=0
    while [ -n "${filler:-}" ] && [ "$i" -lt "$filler" ] 2>/dev/null; do
      separate || break
      printf '{"id":%s,"commit_id":"%s","state":"COMMENTED","body":"history"}' \
        "$((7000 + i))" "ccccccccccccccccccccccccccccccccccccccc"
      i=$((i + 1))
      at=$((at + 1))
    done
    # The receiver's own store, read back as records. `sh` is not asked to
    # parse JSON on the way out: each accepted create appended its rendered
    # record to `reviews.jsonl`, so a scenario that posted one review reads back
    # exactly that review.
    #
    # The store is `{...},{...}` — a record per create with a *leading*
    # separator, which is what one atomic append can carry. Two earlier
    # spellings were not safe and both produced a read-back a concurrent
    # receiver could not answer: a separator written by its own append, which
    # the kernel interleaved into `[,,{...}]`, and a separator decided by an
    # existence probe, which two creates could both read as absent.
    #
    # So each record's leading separator is dropped on the way out and
    # `separate` writes the one the wire needs. Each record is one
    # newline-terminated line, and the builtin `read` loop keeps only the lines
    # it saw end: `read` reports a final line with no newline as a failure, so a
    # record another run is still appending — visible half-copied on tmpfs —
    # ends the loop instead of reaching the answer. An absent or empty store
    # adds nothing, so a pull request with no reviews reads back as `[]`.
    if [ "$ended" -eq 0 ] && [ -s "$dir/reviews.jsonl" ]; then
      while IFS= read -r record; do
        separate || break
        printf '%s' "${record#?}"
        at=$((at + 1))
      done < "$dir/reviews.jsonl"
    fi
  if [ "$shape" = "truncated" ]; then
    # The document opened and the records are valid; what is missing is the
    # close. A decoder that fills that in would invent the end of the list.
    printf '\n'
  else
    printf ']\n'
  fi
elif [ "$is_files" -eq 1 ]; then
  # The changed-file inventory. A scenario can decline to render the diff, or
  # report a list long enough or heavy enough to exceed the adapter's ceilings.
  field files_shape; shape=$val
  if [ "$shape" = "unavailable" ]; then
    printf 'gh: the diff is too large to render (HTTP 406)\n' >&2
    exit 1
  fi
  printf '['
  first=1
  # A hostile build script in the inventory: its patch text would remove the
  # file named by `LGWKS_TEST_MARKER` if anything executed it. Nothing does; the
  # inventory is read as data, and this is the oracle that observes it.
  field build_script; build=$val
  if [ "$build" = "1" ]; then
    first=0
    printf '{"filename":"build.rs","status":"added","additions":1,"deletions":0,"patch":"rm -f $LGWKS_TEST_MARKER; echo EXECUTED"}'
  fi
  field filler_files; filler=$val
  i=0
  while [ -n "${filler:-}" ] && [ "$i" -lt "$filler" ] 2>/dev/null; do
    if [ "$first" -eq 0 ]; then printf ','; fi
    first=0
    printf '{"filename":"filler-%s.txt","status":"modified","additions":1,"deletions":0,"patch":"x"}' "$i"
    i=$((i + 1))
  done
  field diff_bytes; bytes=$val
  if [ -n "${bytes:-}" ] && [ "$bytes" -gt 0 ] 2>/dev/null; then
    if [ "$first" -eq 0 ]; then printf ','; fi
    first=0
    printf '{"filename":"big.rs","status":"modified","additions":1,"deletions":0,"patch":"'
    # `bytes` bytes of patch text as one JSON string. No newline: a raw newline
    # inside a JSON string is invalid, and the adapter would refuse the document
    # rather than exceed the byte ceiling this scenario is about.
    head -c "$bytes" /dev/zero | tr '\0' 'P'
    printf '"}'
  fi
  printf ']\n'
else
  field snapshot_shape; shape=$val
  case "$shape" in
    denied-*)
      # A credential the receiver no longer honours: the line `gh` prints for
      # an expired or cancelled token, with the status the scenario named.
      printf 'gh: Bad credentials (HTTP %s)\n' "${shape#denied-}" >&2
      exit 1
      ;;
  esac
  if [ "$shape" = "moved" ]; then
    # GitHub's renamed-repository answer: a move object naming the canonical
    # location, which the adapter reports rather than silently following.
    printf '{"message":"Moved Permanently","documentation_url":"https://docs.github.com/rest","url":"https://api.github.com/repos/acme/newrepo/pulls/7"}\n'
  else
    # The pull-request read. `head_after_first` models a head that moves between
    # two reads: the first read reports `head_sha`, and every read after it
    # reports `head_after_first` — so a freshness check comparing the two reads
    # sees exactly the change it is meant to catch. The counter is bumped with a
    # single append per call, so two concurrent runs cannot interleave it.
    field head_sha; head=$val
    field head_after_first; moved=$val
    # Only a scenario that names a second head pays for the read counter: the
    # count decides whether this is the first read or a later one, and when no
    # second head is configured the answer is never consulted. The count is the
    # builtin byte count of the appends (see `creates` above), and it still
    # bumps with one atomic append per read, so two concurrent reads cannot
    # interleave it.
    if [ -n "${moved:-}" ]; then
      printf 'x' >> "$dir/reads"
      read_raw=""
      IFS= read -r read_raw < "$dir/reads"
      read_count=${#read_raw}
      if [ "$read_count" -gt 1 ]; then head="$moved"; fi
    fi
    # GitHub's shape: the commits are nested under `head` and `base`.
    field base_sha; base=$val
    printf '{"number":7,"head":{"ref":"feature","sha":"%s"},"base":{"ref":"main","sha":"%s"}}\n' \
      "$head" "$base"
  fi
fi
exit 0
"#;

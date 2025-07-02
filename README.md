# history-shrinker

A tool for reducing history file size, additionally removing secrets (best effort).

I have a very large history file, and as a developer one of the most useful commands is
the following function:
```bash
hgrep () {
    history | grep "$@" | grep -v hgrep
}
```

I use it many times a day. It's like googling your command history.
I set my HISTSIZE and HISTFILESIZE to very large values (50000), so if I know I ran
some esoteric command anytime within the last few years, I will be able to grep for it.

Still, there's a lot of redundant/uninteresting info in there. So I wrote this command,
and run it periodically to clean up the history file.

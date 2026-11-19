package main

import (
	"airhand/p2p"
	"io"
	"log"
	"os"
)

func main() {
	// Third-party libraries must not bypass the category-only control output.
	log.SetOutput(io.Discard)
	if len(os.Args) != 2 || os.Args[1] != "--control" {
		os.Exit(2)
	}
	if p2p.RunControl(os.Stdin, os.Stdout, io.Discard) != nil {
		os.Exit(1)
	}
}

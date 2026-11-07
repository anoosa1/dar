// Packaged services always use HTTPS security; loopback development is explicit.
if(process.argv[2]==="--development")process.argv.splice(2,1);
else process.argv.push("--production");
await import("./dev.mjs");

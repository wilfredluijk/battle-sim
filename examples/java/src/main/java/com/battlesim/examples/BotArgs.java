package com.battlesim.examples;

record BotArgs(String host, int port, String name) {
    static BotArgs parse(String[] args, String defaultName) {
        String host = args.length > 0 ? args[0] : "localhost";
        int port = args.length > 1 ? Integer.parseInt(args[1]) : 7878;
        String name = args.length > 2 ? args[2] : defaultName;
        return new BotArgs(host, port, name);
    }
}

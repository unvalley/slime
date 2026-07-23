on run arguments
    if (count of arguments) is not 2 then
        error "usage: InstallSystem.applescript SOURCE DESTINATION"
    end if

    set sourcePath to item 1 of arguments
    set destinationPath to item 2 of arguments
    if destinationPath is not "/Library/Input Methods/Slime.app" then
        error "refusing unexpected destination: " & destinationPath
    end if

    set installCommand to "/usr/bin/ditto " & quoted form of sourcePath & " " & quoted form of destinationPath
    set ownerCommand to "/usr/sbin/chown -R root:wheel " & quoted form of destinationPath
    do shell script installCommand & " && " & ownerCommand with administrator privileges
end run

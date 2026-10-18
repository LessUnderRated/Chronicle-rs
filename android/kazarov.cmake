# The Android build compiles Kazarov with android/patches applied: what Android needs of the shared
# port before Kazarov carries it. The patched copy lives in the build tree, so the submodule stays
# clean. Sets KZ_ROOT to the copy.

file(GLOB kz_patches CONFIGURE_DEPENDS ${CMAKE_SOURCE_DIR}/android/patches/kazarov-*.patch)
list(SORT kz_patches)
set(kz_copy ${CMAKE_BINARY_DIR}/kazarov)
set(kz_stamp_text "")
foreach(patch IN LISTS kz_patches)
    file(SHA256 ${patch} hash)
    string(APPEND kz_stamp_text "${hash}\n")
endforeach()
execute_process(COMMAND git -C ${KZ_ROOT} rev-parse HEAD OUTPUT_VARIABLE kz_head OUTPUT_STRIP_TRAILING_WHITESPACE)
string(APPEND kz_stamp_text "${kz_head}\n")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS ${kz_patches})

set(kz_stamp_file ${kz_copy}/.android-patches)
set(kz_stamp_old "")
if(EXISTS ${kz_stamp_file})
    file(READ ${kz_stamp_file} kz_stamp_old)
endif()
if(NOT kz_stamp_old STREQUAL kz_stamp_text)
    file(REMOVE_RECURSE ${kz_copy})
    file(COPY ${KZ_ROOT}/ DESTINATION ${kz_copy} PATTERN .git EXCLUDE)
    # Its own repository: inside another's work tree (the Gradle build's is in the checkout) git apply
    # takes a patch's paths from that one's root and skips them, reporting success.
    execute_process(COMMAND git init -q WORKING_DIRECTORY ${kz_copy} RESULT_VARIABLE initialised)
    if(NOT initialised EQUAL 0)
        message(FATAL_ERROR "git init in ${kz_copy} failed")
    endif()
    foreach(patch IN LISTS kz_patches)
        execute_process(COMMAND git apply --verbose --whitespace=nowarn ${patch}
            WORKING_DIRECTORY ${kz_copy} RESULT_VARIABLE applied OUTPUT_QUIET ERROR_VARIABLE applied_files)
        if(applied EQUAL 0 AND NOT applied_files MATCHES "Applied patch")
            set(applied 1)
        endif()
        if(NOT applied EQUAL 0)
            message(FATAL_ERROR "${patch} does not apply to tools/kazarov at ${kz_head}")
        endif()
    endforeach()
    file(WRITE ${kz_stamp_file} "${kz_stamp_text}")
endif()
set(KZ_ROOT ${kz_copy})

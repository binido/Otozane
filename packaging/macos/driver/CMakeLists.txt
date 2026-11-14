cmake_minimum_required(VERSION 3.20)
project(OtozaneDriver CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_OSX_ARCHITECTURES arm64)

# Built by scripts/build-driver.sh from a pinned checkout.
set(LIBASPL_SOURCE_DIR "" CACHE PATH "libASPL source directory")
set(DRIVER_VERSION "0.0.0" CACHE STRING "Driver version")

include(ExternalProject)
ExternalProject_Add(libASPL
  SOURCE_DIR ${LIBASPL_SOURCE_DIR}
  BINARY_DIR ${CMAKE_CURRENT_BINARY_DIR}/libASPL-build
  INSTALL_DIR ${CMAKE_CURRENT_BINARY_DIR}/libASPL-prefix
  CMAKE_ARGS -DCMAKE_INSTALL_PREFIX=<INSTALL_DIR> -DCMAKE_OSX_ARCHITECTURES=arm64
  )

# MODULE builds a Mach-O bundle, CMake wraps it into Otozane.driver.
add_library(Otozane MODULE Driver.cpp)
add_dependencies(Otozane libASPL)
target_include_directories(Otozane PRIVATE ${CMAKE_CURRENT_BINARY_DIR}/libASPL-prefix/include)
target_link_libraries(Otozane PRIVATE
  ${CMAKE_CURRENT_BINARY_DIR}/libASPL-prefix/lib/libASPL.a
  "-framework CoreFoundation"
  "-framework CoreAudio")
set_target_properties(Otozane PROPERTIES
  BUNDLE true
  BUNDLE_EXTENSION driver
  PREFIX ""
  SUFFIX ""
  MACOSX_BUNDLE_INFO_PLIST ${CMAKE_CURRENT_SOURCE_DIR}/Info.plist.in
  MACOSX_BUNDLE_SHORT_VERSION_STRING ${DRIVER_VERSION})

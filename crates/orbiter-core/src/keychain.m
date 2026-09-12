// Signing-key storage in the macOS Keychain. The key never leaves this Mac, is not synchronised
// to iCloud, and is reachable only while the Mac is unlocked. See docs/local-authentication.md.
#import <Foundation/Foundation.h>
#import <Security/Security.h>
#include <string.h>

static NSString *const kService = @"com.orbiter.desktop.signing-key.v1";

static NSMutableDictionary *query(const char *account) {
    NSString *name = [NSString stringWithUTF8String:account ?: ""];
    if ([name length] == 0 || [name length] > 256) return nil;
    return [@{
        (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
        (__bridge id)kSecAttrService: kService,
        (__bridge id)kSecAttrAccount: name,
    } mutableCopy];
}

// 0 stored, 1 bad argument, 2 keychain refused.
int orbiter_keychain_store(const char *account, const unsigned char *bytes, size_t length) {
    @autoreleasepool {
        @try {
            NSMutableDictionary *item = query(account);
            if (!item || !bytes || length == 0 || length > 1024 * 64) return 1;
            NSData *data = [NSData dataWithBytes:bytes length:length];
            // Replacing an existing key is expected: a new key supersedes the old one.
            SecItemDelete((__bridge CFDictionaryRef)item);
            item[(__bridge id)kSecValueData] = data;
            item[(__bridge id)kSecAttrLabel] = @"Orbiter signing key";
            item[(__bridge id)kSecAttrDescription] = @"Development signing key created by Orbiter";
            item[(__bridge id)kSecAttrAccessible] =
                (__bridge id)kSecAttrAccessibleWhenUnlockedThisDeviceOnly;
            item[(__bridge id)kSecAttrSynchronizable] = (__bridge id)kCFBooleanFalse;
            return SecItemAdd((__bridge CFDictionaryRef)item, NULL) == errSecSuccess ? 0 : 2;
        } @catch (NSException *exception) {
            return 2;
        }
    }
}

// 0 found, 1 bad argument, 2 keychain refused, 3 no key stored.
int orbiter_keychain_load(const char *account, unsigned char *buffer, size_t capacity,
                          size_t *written) {
    @autoreleasepool {
        @try {
            NSMutableDictionary *item = query(account);
            if (!item || !buffer || !written) return 1;
            *written = 0;
            item[(__bridge id)kSecReturnData] = (__bridge id)kCFBooleanTrue;
            item[(__bridge id)kSecMatchLimit] = (__bridge id)kSecMatchLimitOne;
            CFTypeRef found = NULL;
            OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)item, &found);
            if (status == errSecItemNotFound) return 3;
            if (status != errSecSuccess || !found) return 2;
            NSData *data = (__bridge_transfer NSData *)found;
            if (data.length == 0 || data.length > capacity) return 2;
            memcpy(buffer, data.bytes, data.length);
            *written = data.length;
            return 0;
        } @catch (NSException *exception) {
            return 2;
        }
    }
}

// 0 removed or absent, 1 bad argument, 2 keychain refused.
int orbiter_keychain_delete(const char *account) {
    @autoreleasepool {
        @try {
            NSMutableDictionary *item = query(account);
            if (!item) return 1;
            OSStatus status = SecItemDelete((__bridge CFDictionaryRef)item);
            return (status == errSecSuccess || status == errSecItemNotFound) ? 0 : 2;
        } @catch (NSException *exception) {
            return 2;
        }
    }
}

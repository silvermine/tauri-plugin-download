//
//  DownloadsView.swift
//  DownloadManagerExample
//

import Combine
import SwiftUI
import DownloadManagerKit

struct PendingDownload: Identifiable {
   var id: String { path.path }
   let url: URL
   let path: URL
}

struct DownloadsView: View {
   @StateObject private var manager = DownloadManager.shared
   @State private var downloadUrl: String = ""
   @State private var autoCreate: Bool = true
   @State private var pendingDownloads: [PendingDownload] = []

   init() {
      Task {
         for await download in DownloadManager.shared.changed {
            print("[\(download.path)] \(download.status) - \(String(format: "%.0f", download.progress))%")
         }
      }
   }

   var body: some View {
      NavigationView {
         VStack {
            Text("Enter a URL to download and click Get.")
               .font(.subheadline)
               .foregroundColor(.secondary)
               .padding(.top)
            
            HStack {
               TextField("https://example.com/file.zip", text: $downloadUrl)
                  .textFieldStyle(RoundedBorderTextFieldStyle())
                  .autocapitalization(.none)
                  .disableAutocorrection(true)
                  .keyboardType(.URL)
               
               Button(action: getDownload) {
                  Text("Get")
                     .padding(.horizontal, 10)
                     .padding(.vertical, 5)
                     .background(Color.blue)
                     .foregroundColor(.white)
                     .cornerRadius(8)
               }
               .disabled(downloadUrl.isEmpty)
            }
            .padding(.horizontal)
            
            Toggle("Auto-create", isOn: $autoCreate)
               .padding(.horizontal)

            List {
               ForEach(pendingDownloads) { pending in
                  PendingDownloadRowView(pending: pending, manager: manager, onCreated: {
                     pendingDownloads.removeAll { $0.path == pending.path }
                  })
               }
               ForEach(manager.downloads) { item in
                  DownloadRowView(item: item, manager: manager)
               }
            }
         }
         .navigationTitle("Downloads")
      }
   }

   private func getDownload() {
      guard !downloadUrl.isEmpty,
            let url = URL(string: downloadUrl),
            url.scheme != nil && url.host != nil else {
         return
      }

      let filename = url.lastPathComponent
      let path = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent(filename)
      
      let download = manager.get(path: path)
      
      if download.status == .pending {
         if autoCreate {
            _ = manager.create(path: path, url: url)
         } else {
            pendingDownloads.append(PendingDownload(url: url, path: path))
         }
      }
      
      downloadUrl = ""
   }
}

struct PendingDownloadRowView: View {
   let pending: PendingDownload
   let manager: DownloadManager
   let onCreated: () -> Void
   
   var body: some View {
      VStack(alignment: .leading) {
         Text(pending.path.lastPathComponent)
            .font(.headline)
         Text("Status: pending")
            .font(.caption)
            .foregroundColor(.secondary)
         
         Button(action: {
            _ = manager.create(path: pending.path, url: pending.url)
            onCreated()
         }) {
            Text("Create")
               .padding(8)
               .background(Color.green.opacity(0.2))
               .cornerRadius(8)
         }.buttonStyle(PlainButtonStyle())
      }
      .padding(.vertical, 4)
   }
}

struct DownloadRowView: View {
   @ObservedObject var item: DownloadItem
   let manager: DownloadManager
   
   var body: some View {
      VStack(alignment: .leading) {
         Text(item.path.lastPathComponent)
            .font(.headline)
         ProgressView(value: item.progress / 100)
            .progressViewStyle(LinearProgressViewStyle())
         Text("Status: \(item.status.rawValue)")
            .font(.caption)
            .foregroundColor(.secondary)
         
         switch item.status {
         case .idle:
            HStack(spacing: 8) {
               Button(action: { _ = try? manager.start(path: item.path) }) {
                  Text("Start")
                     .padding(8)
                     .background(Color.blue.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
               Button(action: { _ = try? manager.cancel(path: item.path) }) {
                  Text("Cancel")
                     .padding(8)
                     .background(Color.red.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
            }
         case .inProgress:
            HStack(spacing: 8) {
               Button(action: { _ = try? manager.pause(path: item.path) }) {
                  Text("Pause")
                     .padding(8)
                     .background(Color.blue.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
               Button(action: { _ = try? manager.cancel(path: item.path) }) {
                  Text("Cancel")
                     .padding(8)
                     .background(Color.red.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
            }
         case .paused:
            HStack(spacing: 8) {
               Button(action: { _ = try? manager.resume(path: item.path) }) {
                  Text("Resume")
                     .padding(8)
                     .background(Color.blue.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
               Button(action: { _ = try? manager.cancel(path: item.path) }) {
                  Text("Cancel")
                     .padding(8)
                     .background(Color.red.opacity(0.2))
                     .cornerRadius(8)
               }.buttonStyle(PlainButtonStyle())
            }
         default:
            EmptyView()
         }
      }
      .padding(.vertical, 4)
   }
}
